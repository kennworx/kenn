//! Dead-CSS lookups for the `check_css` MCP report (index-css Group 9).
//!
//! Two graph queries over the stylesheet corpus:
//! - **orphan class** — a `css_class` node with no inbound `uses_css_class` and
//!   no inbound `extends_rule` (used by no code and extended by no rule). Only
//!   meaningful once class-usage mining has run, so it is gated on the presence
//!   of at least one `uses_css_class` edge (the `usage_mining_on` signal).
//! - **orphan stylesheet** — a `module` node nothing `@import`s/`@use`s and none
//!   of whose selectors are used (a dead sheet).
//!
//! Output is bounded: at most `limit` rows total (classes first, then sheets),
//! with the full per-category counts returned separately so the caller can
//! report truncation.

use super::super::super::codes::edge_kind_code;
use super::projection::{be, SqliteConnRef};
use crate::api::types::{CssHealthCounts, CssHealthRow, DbError};
use kenn_model::EdgeKind;

impl SqliteConnRef<'_> {
    /// Scan the index for dead CSS. `want_classes`/`want_sheets` select the
    /// categories; orphan-class is additionally suppressed when no
    /// `uses_css_class` edge exists (usage mining off). Returns up to `limit`
    /// rows (classes first) plus the full per-category counts.
    pub(crate) fn scan_css_health(
        &self,
        want_classes: bool,
        want_sheets: bool,
        limit: u32,
    ) -> Result<(Vec<CssHealthRow>, CssHealthCounts), DbError> {
        let uses = edge_kind_code(EdgeKind::UsesCssClass);
        let extends = edge_kind_code(EdgeKind::ExtendsRule);
        let imports = edge_kind_code(EdgeKind::Imports);
        let defined_in = edge_kind_code(EdgeKind::DefinedIn);

        let usage_edges: i64 = self
            .conn
            .prepare_cached("SELECT count(*) FROM edges WHERE kind = ?1")
            .map_err(be)?
            .query_row([i64::from(uses)], |r| r.get(0))
            .map_err(be)?;
        let usage_mining_on = usage_edges > 0;

        let mut counts = CssHealthCounts {
            usage_mining_on,
            ..CssHealthCounts::default()
        };
        let mut rows: Vec<CssHealthRow> = Vec::new();

        // Predicate fragments built from trusted small ints / literals.
        let class_unused = format!(
            "s.kind = 'css_class' AND s.external = 0 AND NOT EXISTS (\
               SELECT 1 FROM edges e WHERE e.target_id = s.id AND e.kind IN ({uses},{extends}))"
        );
        let sheet_dead = format!(
            "s.kind = 'module' AND s.language IN ('css','sass') AND s.external = 0 \
             AND NOT EXISTS (SELECT 1 FROM edges ie WHERE ie.target_id = s.id AND ie.kind = {imports}) \
             AND NOT EXISTS (\
               SELECT 1 FROM edges de \
               JOIN edges ue ON ue.target_id = de.src_id AND ue.kind IN ({uses},{extends}) \
               WHERE de.target_id = s.id AND de.kind = {defined_in})"
        );

        if want_classes && usage_mining_on {
            counts.orphan_classes = self.count_where(&class_unused)?;
            self.collect_health("orphan_class", &class_unused, true, limit, &mut rows)?;
        }
        if want_sheets {
            counts.orphan_stylesheets = self.count_where(&sheet_dead)?;
            let remaining = limit.saturating_sub(u32::try_from(rows.len()).unwrap_or(u32::MAX));
            if remaining > 0 {
                self.collect_health(
                    "orphan_stylesheet",
                    &sheet_dead,
                    false,
                    remaining,
                    &mut rows,
                )?;
            }
        }
        Ok((rows, counts))
    }

    /// `count(*)` over `symbols s` satisfying `predicate`.
    fn count_where(&self, predicate: &str) -> Result<u64, DbError> {
        let n: i64 = self
            .conn
            .prepare_cached(&format!("SELECT count(*) FROM symbols s WHERE {predicate}"))
            .map_err(be)?
            .query_row([], |r| r.get(0))
            .map_err(be)?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Append up to `limit` `(pub_id, location)` rows for symbols satisfying
    /// `predicate` into `rows`, tagged with `category`. `with_line` includes the
    /// def line in the location (`path#L<line>`); else just the path.
    fn collect_health(
        &self,
        category: &str,
        predicate: &str,
        with_line: bool,
        limit: u32,
        rows: &mut Vec<CssHealthRow>,
    ) -> Result<(), DbError> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT s.pub_id, f.path, d.start_line FROM symbols s \
                 JOIN defs d ON d.sym_id = s.id \
                 JOIN files f ON f.id = d.file_id \
                 WHERE {predicate} GROUP BY s.id ORDER BY s.pub_id LIMIT ?1"
            ))
            .map_err(be)?;
        let mapped = stmt
            .query_map([i64::from(limit)], |r| {
                let pub_id: String = r.get(0)?;
                let path: String = r.get(1)?;
                let line: i64 = r.get(2)?;
                let location = if with_line {
                    format!("{path}#L{line}")
                } else {
                    path
                };
                Ok(CssHealthRow {
                    category: category.to_string(),
                    pub_id,
                    location: Some(location),
                })
            })
            .map_err(be)?;
        for row in mapped {
            rows.push(row.map_err(be)?);
        }
        Ok(())
    }
}
