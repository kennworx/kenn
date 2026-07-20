//! Point-fetch and catalog inherent methods on [`SqliteConn`].

use rusqlite::OptionalExtension;

use super::projection::{
    be, col_u32, package_from_row, symbol_from_row, SqliteConnRef, COUNT_TABLES, SYMBOL_COLS,
};
use crate::api::types::{
    DbError, DefLineRow, DefRow, PackageRow, StatRow, SymbolDocsRow, SymbolRow,
};

impl SqliteConnRef<'_> {
    pub(crate) fn fetch_symbol_by_short_id(&self, id: u32) -> Result<Option<SymbolRow>, DbError> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT {SYMBOL_COLS} FROM symbols WHERE id=?1 LIMIT 1"
            ))
            .map_err(be)?;
        stmt.query_row([i64::from(id)], symbol_from_row)
            .optional()
            .map_err(be)
    }

    pub(crate) fn fetch_symbol_pub_id(&self, id: u32) -> Result<Option<String>, DbError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT pub_id FROM symbols WHERE id=?1 LIMIT 1")
            .map_err(be)?;
        stmt.query_row([i64::from(id)], |r| r.get::<_, String>(0))
            .optional()
            .map_err(be)
    }

    pub(crate) fn fetch_symbol(
        &self,
        language: &str,
        pub_id: &str,
    ) -> Result<Option<SymbolRow>, DbError> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT {SYMBOL_COLS} FROM symbols WHERE language=?1 AND pub_id=?2 LIMIT 1"
            ))
            .map_err(be)?;
        stmt.query_row(rusqlite::params![language, pub_id], symbol_from_row)
            .optional()
            .map_err(be)
    }

    pub(crate) fn fetch_symbol_docs_row(&self, id: u32) -> Result<Option<SymbolDocsRow>, DbError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT sig, doc FROM symbol_docs WHERE sym_id=?1 LIMIT 1")
            .map_err(be)?;
        stmt.query_row([i64::from(id)], |r| {
            Ok(SymbolDocsRow {
                sig: r.get(0)?,
                doc: r.get(1)?,
            })
        })
        .optional()
        .map_err(be)
    }

    pub(crate) fn fetch_defs(&self, id: u32) -> Result<Vec<DefRow>, DbError> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT file_id, start_line, start_col, end_line, end_col, \
                 body_start_line, body_end_line FROM defs WHERE sym_id=?1",
            )
            .map_err(be)?;
        let rows = stmt
            .query_map([i64::from(id)], |r| {
                Ok(DefRow {
                    file_id: col_u32(r, 0)?,
                    start_line: col_u32(r, 1)?,
                    start_col: col_u32(r, 2)?,
                    end_line: col_u32(r, 3)?,
                    end_col: col_u32(r, 4)?,
                    body_start_line: col_u32(r, 5)?,
                    body_end_line: col_u32(r, 6)?,
                })
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    pub(crate) fn fetch_def_lines(&self, id: u32) -> Result<Vec<DefLineRow>, DbError> {
        Ok(self
            .fetch_defs(id)?
            .into_iter()
            .map(|d| DefLineRow {
                file_id: d.file_id,
                start_line: d.start_line,
                end_line: d.end_line,
                body_start_line: d.body_start_line,
                body_end_line: d.body_end_line,
            })
            .collect())
    }

    pub(crate) fn fetch_package(&self, id: u32) -> Result<Option<PackageRow>, DbError> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT id, name, version, manager, external FROM packages WHERE id=?1 LIMIT 1",
            )
            .map_err(be)?;
        stmt.query_row([i64::from(id)], package_from_row)
            .optional()
            .map_err(be)
    }

    pub(crate) fn fetch_file_path(&self, id: u32) -> Result<Option<String>, DbError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT path FROM files WHERE id=?1 LIMIT 1")
            .map_err(be)?;
        stmt.query_row([i64::from(id)], |r| r.get::<_, String>(0))
            .optional()
            .map_err(be)
    }

    pub(crate) fn fetch_file_short_id(&self, path: &str) -> Result<Option<u32>, DbError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM files WHERE path=?1 LIMIT 1")
            .map_err(be)?;
        stmt.query_row([path], |r| col_u32(r, 0))
            .optional()
            .map_err(be)
    }

    pub(crate) fn distinct_languages(&self) -> Result<Vec<String>, DbError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT DISTINCT language FROM symbols ORDER BY language")
            .map_err(be)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    pub(crate) fn distinct_packages(&self) -> Result<Vec<String>, DbError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT DISTINCT name FROM packages ORDER BY name")
            .map_err(be)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Row count for a standard graph table. Unknown table names return 0
    /// (mirroring the prior resident-map behaviour); the [`COUNT_TABLES`]
    /// whitelist keeps the interpolated table name SQL-safe.
    pub(crate) fn count_table(&self, table: &str) -> Result<u64, DbError> {
        if !COUNT_TABLES.contains(&table) {
            return Ok(0);
        }
        let n: i64 = self
            .conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .map_err(be)?;
        u64::try_from(n).map_err(|e| DbError::Backend(format!("negative count: {e}")))
    }

    /// All build-time `stats` rows (build-time-stats). Tiny table (a few rows
    /// per language/manager + the graph counters), read in one shot.
    pub(crate) fn stats(&self) -> Result<Vec<StatRow>, DbError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT scope, key, subset, metric, value FROM stats")
            .map_err(be)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(StatRow {
                    scope: r.get(0)?,
                    key: r.get(1)?,
                    subset: r.get(2)?,
                    metric: r.get(3)?,
                    value: r.get(4)?,
                })
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// A resolver over this snapshot's code-node ids, for findings read-time
    /// staleness — one build, then O(1) `contains`. The `pub_id` column is
    /// already the canonical code-node id (it carries the language short-code,
    /// e.g. `rs:foo::bar`, `cs:Ns.Type`) — the same form `find_symbol` returns
    /// and an agent stores in `parent_ids`. It must NOT be re-prefixed with the
    /// `language` column (`rust`/`csharp`/…) or every code-cited finding folds
    /// to stale because the doubled id (`rust:rs:foo`) never matches.
    pub(crate) fn code_node_resolver(
        &self,
    ) -> Result<super::super::super::findings::CodeGraphNodeResolver, DbError> {
        let mut q = self
            .conn
            .prepare("SELECT pub_id FROM symbols")
            .map_err(be)?;
        let rows = q.query_map([], |r| r.get::<_, String>(0)).map_err(be)?;
        let ids = rows.collect::<Result<_, _>>().map_err(be)?;
        Ok(super::super::super::findings::CodeGraphNodeResolver::new(
            ids,
        ))
    }
}
