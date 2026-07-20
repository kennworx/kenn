//! Code-graph lookups for md→code link resolution (index-markdown Group 6).
//!
//! Two narrow, name-anchored queries the markdown ingest's post-code barrier
//! drives through [`crate::db::DbReader`] (not the [`crate::api::Reader`] trait —
//! they serve the indexer, not the MCP hot path). Both exclude `external` rows
//! and the `markdown` corpus itself, so a markdown link only ever resolves to
//! real in-workspace code.

use super::super::super::codes::{edge_kind_code, edge_kind_name, link_grade_name};
use super::projection::{be, col_u32, file_from_row, SqliteConnRef};
use crate::api::types::{CodeSymbolHit, DbError, FileRow, LinkDiagnosticRow};
use kenn_model::EdgeKind;

impl SqliteConnRef<'_> {
    /// Code FILE rows whose filename (basename) equals `basename` — the exact
    /// root-level path or any nested `…/<basename>`. Excludes external and
    /// markdown files. `basename` is matched literally (LIKE metacharacters are
    /// escaped).
    pub(crate) fn files_by_basename(&self, basename: &str) -> Result<Vec<FileRow>, DbError> {
        let esc = basename
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let suffix = format!("%/{esc}");
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT id,path,language,test,external FROM files \
                 WHERE external = 0 AND language <> 'markdown' \
                 AND (path = ?1 OR path LIKE ?2 ESCAPE '\\')",
            )
            .map_err(be)?;
        let rows = stmt
            .query_map(rusqlite::params![basename, suffix], file_from_row)
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Code SYMBOL rows whose short (whole) name equals `name`, case-insensitive
    /// (the indexed `name_lower` column). Each hit carries its qualified `pub_id`
    /// (for qualifier-drift grading) and one def's file path (for locality).
    /// Excludes external and markdown symbols.
    pub(crate) fn symbols_by_short_name(&self, name: &str) -> Result<Vec<CodeSymbolHit>, DbError> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT s.id, s.pub_id, f.path FROM symbols s \
                 JOIN defs d ON d.sym_id = s.id \
                 JOIN files f ON f.id = d.file_id \
                 WHERE s.name_lower = ?1 AND s.external = 0 AND s.language <> 'markdown' \
                 GROUP BY s.id",
            )
            .map_err(be)?;
        let rows = stmt
            .query_map(rusqlite::params![name.to_lowercase()], |r| {
                Ok(CodeSymbolHit {
                    id: col_u32(r, 0)?,
                    qualified: r.get(1)?,
                    relpath: r.get(2)?,
                })
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Every non-exact markdown link, for the `check_links` report — the read
    /// path for the `link_grade` edge column (the first edge-payload reader).
    /// Scans `links_to`/`embeds`/`links_to_file` edges whose source is a
    /// markdown node and whose grade is not `exact`. The target is hydrated by
    /// edge kind: `links_to_file` reads the files table (sound despite the
    /// file/symbol id collision); the others read symbols (a resolved md/code
    /// symbol, or a dangling external stub whose `pub_id` holds the written
    /// target).
    /// `grade_codes` (when `Some`) restricts to those link-grade discriminants;
    /// `None` lists every non-exact grade. At most `limit` rows are materialized
    /// (the worst link corpus shouldn't flood the MCP response); the returned
    /// `u64` is the *full* matching count so the caller can report truncation.
    pub(crate) fn scan_link_diagnostics(
        &self,
        grade_codes: Option<&[u8]>,
        limit: u32,
    ) -> Result<(Vec<LinkDiagnosticRow>, u64), DbError> {
        let links_to_file = i64::from(edge_kind_code(EdgeKind::LinksToFile));
        // Kind + grade predicates built from trusted small ints (no injection).
        let kinds = [EdgeKind::LinksTo, EdgeKind::Embeds, EdgeKind::LinksToFile]
            .map(|k| edge_kind_code(k).to_string())
            .join(",");
        let grade_pred = match grade_codes {
            Some(codes) if !codes.is_empty() => {
                let list = codes
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                format!("e.link_grade IN ({list})")
            }
            // No (or empty) filter → every non-exact grade.
            _ => "e.link_grade IS NOT NULL AND e.link_grade <> 0".to_string(),
        };
        let from_where = format!(
            "FROM edges e \
             JOIN symbols s ON s.id = e.src_id AND s.language = 'markdown' \
             WHERE e.kind IN ({kinds}) AND {grade_pred}"
        );

        let total: u64 = self
            .conn
            .prepare_cached(&format!("SELECT count(*) {from_where}"))
            .map_err(be)?
            .query_row([], |r| r.get::<_, i64>(0))
            .map_err(be)?
            .try_into()
            .unwrap_or(0);

        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT s.pub_id, sf.path, sd.start_line, e.kind, e.link_grade, \
                        tsym.pub_id, tf.path \
                 FROM edges e \
                 JOIN symbols s ON s.id = e.src_id AND s.language = 'markdown' \
                 LEFT JOIN defs sd ON sd.sym_id = e.src_id \
                 LEFT JOIN files sf ON sf.id = sd.file_id \
                 LEFT JOIN symbols tsym ON tsym.id = e.target_id \
                 LEFT JOIN files tf ON tf.id = e.target_id \
                 WHERE e.kind IN ({kinds}) AND {grade_pred} \
                 ORDER BY s.pub_id LIMIT ?1"
            ))
            .map_err(be)?;
        let rows = stmt
            .query_map(rusqlite::params![i64::from(limit)], |r| {
                let src_pub_id: String = r.get(0)?;
                let src_path: Option<String> = r.get(1)?;
                let src_line: Option<i64> = r.get(2)?;
                let kind_code: i64 = r.get(3)?;
                let grade_code: i64 = r.get(4)?;
                let tgt_sym: Option<String> = r.get(5)?;
                let tgt_file: Option<String> = r.get(6)?;
                #[expect(
                    clippy::cast_sign_loss,
                    clippy::cast_possible_truncation,
                    reason = "edge kind/grade are small non-negative codes stored as i64"
                )]
                let kind = edge_kind_name(kind_code as u32);
                #[expect(
                    clippy::cast_sign_loss,
                    clippy::cast_possible_truncation,
                    reason = "edge kind/grade are small non-negative codes stored as i64"
                )]
                let grade = link_grade_name(grade_code as u8).to_string();
                let target = if kind_code == links_to_file {
                    tgt_file.unwrap_or_default()
                } else {
                    tgt_sym.unwrap_or_default()
                };
                let location = src_path.map(|p| match src_line {
                    Some(l) => format!("{p}#L{l}"),
                    None => p,
                });
                Ok(LinkDiagnosticRow {
                    src_pub_id,
                    location,
                    kind,
                    grade,
                    target,
                })
            })
            .map_err(be)?;
        let out: Vec<LinkDiagnosticRow> = rows.collect::<Result<_, _>>().map_err(be)?;
        Ok((out, total))
    }
}
