//! Graph-traversal inherent methods on [`SqliteConn`], served live from the
//! `edges`/`defs` indexes (`edges_src`, `edges_tgt`, `defs_file`).

use rusqlite::{Connection, OptionalExtension};

use super::super::super::codes::{edge_kind_code, parse_edge_relation};
use super::projection::{
    be, col_u32, fetch_symbols_by_ids, file_from_row, passes_filter, SqliteConnRef,
};
use crate::api::types::{DbError, FileRow, RowNarrow, SymbolRow};
use kenn_model::EdgeKind;

/// Which endpoint of `edges` is the pivot vs. the neighbour.
#[derive(Clone, Copy)]
enum Direction {
    /// `source == pivot`; neighbours are `target_id`.
    Outbound,
    /// `target == pivot`; neighbours are `src_id`.
    Inbound,
}

impl Direction {
    /// `(pivot_column, neighbour_column)` for the `edges` query.
    fn columns(self) -> (&'static str, &'static str) {
        match self {
            Direction::Outbound => ("src_id", "target_id"),
            Direction::Inbound => ("target_id", "src_id"),
        }
    }
}

impl SqliteConnRef<'_> {
    /// Symbols connected from `pivot` over `relation` (`source == pivot`).
    pub(crate) fn list_outbound(
        &self,
        pivot: u32,
        relation: &str,
        limit: u32,
        cursor_after: Option<u32>,
        narrow: &RowNarrow,
    ) -> Result<(Vec<SymbolRow>, u64), DbError> {
        self.list_edges(
            Direction::Outbound,
            pivot,
            relation,
            limit,
            cursor_after,
            narrow,
        )
    }

    /// Symbols connected to `pivot` over `relation` (`target == pivot`).
    pub(crate) fn list_inbound(
        &self,
        pivot: u32,
        relation: &str,
        limit: u32,
        cursor_after: Option<u32>,
        narrow: &RowNarrow,
    ) -> Result<(Vec<SymbolRow>, u64), DbError> {
        self.list_edges(
            Direction::Inbound,
            pivot,
            relation,
            limit,
            cursor_after,
            narrow,
        )
    }

    /// Shared traversal body: the distinct neighbour ids of `pivot` over the
    /// edge kind, with id `> cursor_after`, in ascending id order (covered by
    /// the `edges_src`/`edges_tgt` indexes). `total` is the count of those
    /// distinct neighbours (pre external/test filter, matching the prior CSR
    /// behaviour); the returned rows are hydrated, filtered, and truncated to
    /// `limit`.
    fn list_edges(
        &self,
        direction: Direction,
        pivot: u32,
        relation: &str,
        limit: u32,
        cursor_after: Option<u32>,
        narrow: &RowNarrow,
    ) -> Result<(Vec<SymbolRow>, u64), DbError> {
        let Some(kind) = parse_edge_relation(relation) else {
            return Err(DbError::Backend(format!("unknown relation: {relation}")));
        };
        let want = i64::from(edge_kind_code(kind));
        let after = i64::from(cursor_after.unwrap_or(0));
        let (pivot_col, other_col) = direction.columns();

        let conn = self.conn;
        let others: Vec<u32> = {
            let mut stmt = conn
                .prepare_cached(&format!(
                    "SELECT DISTINCT {other_col} FROM edges \
                     WHERE {pivot_col}=?1 AND kind=?2 AND {other_col}>?3 ORDER BY {other_col}"
                ))
                .map_err(be)?;
            let rows = stmt
                .query_map(rusqlite::params![i64::from(pivot), want, after], |r| {
                    col_u32(r, 0)
                })
                .map_err(be)?;
            rows.collect::<Result<_, _>>().map_err(be)?
        };
        let total = others.len() as u64;
        let symbols = fetch_symbols_by_ids(conn, &others)?;
        // Package names resolve to ids ONCE per traversal, not per row.
        let pkg_ids = resolve_package_ids(conn, narrow)?;

        let mut out = Vec::new();
        for id in others {
            if out.len() >= limit as usize {
                break;
            }
            if let Some(s) = symbols.get(&id) {
                if passes_filter(s, narrow, pkg_ids.as_ref()) {
                    out.push(s.clone());
                }
            }
        }
        Ok((out, total))
    }

    /// Symbols whose declaration span encloses `line` in `file`, tightest first.
    pub(crate) fn find_at_location(&self, file: u32, line: u32) -> Result<Vec<SymbolRow>, DbError> {
        let conn = self.conn;
        let mut hits: Vec<(u32, u32, u32)> = {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT start_line, end_line, sym_id FROM defs \
                     WHERE file_id=?1 AND start_line<=?2 AND ?2<=end_line",
                )
                .map_err(be)?;
            let rows = stmt
                .query_map(rusqlite::params![i64::from(file), i64::from(line)], |r| {
                    Ok((col_u32(r, 0)?, col_u32(r, 1)?, col_u32(r, 2)?))
                })
                .map_err(be)?;
            rows.collect::<Result<_, _>>().map_err(be)?
        };
        // Tightest span first, then by symbol id — same order as the prior
        // in-RAM path (SQLite has no defined row order without this).
        hits.sort_by(|a, b| {
            (a.1.saturating_sub(a.0))
                .cmp(&(b.1.saturating_sub(b.0)))
                .then(a.2.cmp(&b.2))
        });
        let mut seen = std::collections::HashSet::new();
        let ordered: Vec<u32> = hits
            .into_iter()
            .filter(|(_, _, sym)| seen.insert(*sym))
            .map(|(_, _, sym)| sym)
            .collect();
        let symbols = fetch_symbols_by_ids(conn, &ordered)?;
        Ok(ordered
            .into_iter()
            .filter_map(|sym| symbols.get(&sym).cloned())
            .collect())
    }

    /// Files a module `contains` (the `Contains` edge to file ids), in
    /// ascending file-id order, with id `> cursor_after`.
    pub(crate) fn list_module_files(
        &self,
        module: u32,
        limit: u32,
        cursor_after: Option<u32>,
    ) -> Result<(Vec<FileRow>, u64), DbError> {
        let want = i64::from(edge_kind_code(EdgeKind::Contains));
        let after = i64::from(cursor_after.unwrap_or(0));
        let conn = self.conn;
        let ids: Vec<u32> = {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT DISTINCT target_id FROM edges \
                     WHERE src_id=?1 AND kind=?2 AND target_id>?3 ORDER BY target_id",
                )
                .map_err(be)?;
            let rows = stmt
                .query_map(rusqlite::params![i64::from(module), want, after], |r| {
                    col_u32(r, 0)
                })
                .map_err(be)?;
            rows.collect::<Result<_, _>>().map_err(be)?
        };
        let total = ids.len() as u64;
        let out = hydrate_files(conn, &ids, limit)?;
        Ok((out, total))
    }
}

/// Read up to `limit` file rows by id (in `ids` order) on `conn`.
fn hydrate_files(conn: &Connection, ids: &[u32], limit: u32) -> Result<Vec<FileRow>, DbError> {
    let mut stmt = conn
        .prepare_cached("SELECT id, path, language, test, external FROM files WHERE id=?1 LIMIT 1")
        .map_err(be)?;
    let mut out = Vec::new();
    for &id in ids.iter().take(limit as usize) {
        if let Some(f) = stmt
            .query_row([i64::from(id)], file_from_row)
            .optional()
            .map_err(be)?
        {
            out.push(f);
        }
    }
    Ok(out)
}

/// Resolve `narrow`'s package NAMES to ids, once per traversal.
///
/// `SymbolRow` carries `pkg_id`, not the package name, so a name filter needs
/// one lookup — done here rather than per row, and skipped entirely when no
/// package narrowing was asked for. An unknown name yields an empty set, which
/// correctly matches nothing rather than silently matching everything.
fn resolve_package_ids(
    conn: &Connection,
    narrow: &RowNarrow,
) -> Result<Option<std::collections::HashSet<u32>>, DbError> {
    let Some(names) = narrow.packages.as_ref() else {
        return Ok(None);
    };
    let mut stmt = conn
        .prepare_cached("SELECT id FROM packages WHERE name=?1")
        .map_err(be)?;
    let mut ids = std::collections::HashSet::new();
    for n in names {
        let rows = stmt
            .query_map(rusqlite::params![n], |r| col_u32(r, 0))
            .map_err(be)?;
        for id in rows {
            ids.insert(id.map_err(be)?);
        }
    }
    Ok(Some(ids))
}
