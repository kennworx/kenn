//! `SQLite` ingest writer (replace-lance-with-sqlite, tasks 2.1 / 2.3).
//!
//! Maps a [`WriteBatch`] of typed records 1:1 into the `code.db` tables in
//! one transaction. The knowledge-row derivation (which joins symbols+docs
//! across batches) and FTS5/`vec0` population happen at `finalize` from the
//! fully-populated graph tables — not here — so cross-batch ordering of a
//! symbol and its doc rows doesn't matter (design D2/D5).

use std::path::Path;

use rusqlite::{params, Connection};

use kenn_model::EdgeProperties;

use crate::api::types::{DbError, WriterOptions};
use crate::api::WriteBatch;

use super::super::super::codes::{
    edge_kind_code, field_op_code, import_kind_code, iso_source_code, link_grade_code,
};
use super::super::schema;

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a map_err fn pointer, which passes the error by value"
)]
pub(super) fn be(e: rusqlite::Error) -> DbError {
    DbError::Backend(format!("sqlite: {e}"))
}

/// Read column `idx` as a `u32`. `SQLite` stores ids as signed `i64`; kenn ids
/// are non-negative and bounded to `u32`, so the cast is lossless.
#[expect(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "SQLite stores ids as i64; kenn ids are non-negative u32, so the cast is lossless"
)]
pub(super) fn col_u32(r: &rusqlite::Row, idx: usize) -> rusqlite::Result<u32> {
    Ok(r.get::<_, i64>(idx)? as u32)
}

/// Read column `idx` as a `u16`. `SQLite` stores the `nargs`/`targs` arity
/// counts as `i64`; they are non-negative and fit a `u16` (widened from `u8`
/// after a 257-arg method in Newtonsoft.Json overflowed the byte), so the cast
/// is lossless.
#[expect(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "SQLite stores arity counts as i64; they are non-negative and fit a u16, so the cast is lossless"
)]
pub(super) fn col_u16(r: &rusqlite::Row, idx: usize) -> rusqlite::Result<u16> {
    Ok(r.get::<_, i64>(idx)? as u16)
}

/// Build-time pragmas: a snapshot DB is written once then published by an
/// atomic rename, so per-insert durability is wasted work.
const BUILD_PRAGMAS: &str = "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;";

/// The flattened `(field_op, import_kind, corr_source, corr_generator,
/// corr_canon_id, link_grade, link_relation)` nullable columns of one
/// `edges` row.
type EdgeFlatCols = (
    Option<u8>,
    Option<u8>,
    Option<u8>,
    Option<String>,
    Option<u32>,
    Option<u8>,
    Option<String>,
);

/// Flatten an `EdgeProperties` payload into the `edges` table's nullable
/// columns `(field_op, import_kind, corr_source, corr_generator,
/// corr_canon_id, link_grade, link_relation)`. Split out of `write_batch` so
/// its per-variant branching does not inflate the writer's complexity.
fn flatten_edge_properties(props: &EdgeProperties) -> EdgeFlatCols {
    match props {
        EdgeProperties::FieldAccess { op } => {
            (Some(field_op_code(*op)), None, None, None, None, None, None)
        }
        EdgeProperties::Imports { kind } => (
            None,
            Some(import_kind_code(*kind)),
            None,
            None,
            None,
            None,
            None,
        ),
        EdgeProperties::CorrespondsTo {
            source,
            generator,
            canonical,
        } => (
            None,
            None,
            Some(iso_source_code(*source)),
            Some(generator.clone()),
            Some(*canonical),
            None,
            None,
        ),
        EdgeProperties::LinksTo { grade, relation } => (
            None,
            None,
            None,
            None,
            None,
            Some(link_grade_code(*grade)),
            Some(relation.clone()),
        ),
        EdgeProperties::Embeds { grade }
        | EdgeProperties::LinksToFile { grade }
        | EdgeProperties::UsesCssClass { grade }
        | EdgeProperties::ExtendsRule { grade } => (
            None,
            None,
            None,
            None,
            None,
            Some(link_grade_code(*grade)),
            None,
        ),
        _ => (None, None, None, None, None, None, None),
    }
}

/// Writes one snapshot's three `SQLite` databases. Created at `runs/<ts>/`
/// during an index run; published by the lifecycle's per-store rename.
pub(crate) struct SqliteWriter {
    pub(super) graph: Connection,
    pub(super) knowledge: Connection,
    pub(super) options: WriterOptions,
}

impl SqliteWriter {
    /// Create the three databases under `dir` with their schemas applied.
    pub(crate) fn create(dir: &Path, options: WriterOptions) -> Result<Self, DbError> {
        super::super::ensure_vec_extension();
        std::fs::create_dir_all(dir).map_err(DbError::Io)?;
        let open = |name: &str, ddl: &dyn Fn(&Connection) -> rusqlite::Result<()>| {
            let c = Connection::open(dir.join(name)).map_err(be)?;
            c.execute_batch(BUILD_PRAGMAS).map_err(be)?;
            ddl(&c).map_err(be)?;
            Ok::<_, DbError>(c)
        };
        Ok(Self {
            graph: open(crate::db::names::CODE_DB, &schema::create_graph)?,
            knowledge: open(crate::db::names::VECTOR_DB, &schema::create_knowledge)?,
            options,
        })
    }

    /// The options this writer was created with.
    pub(crate) fn options(&self) -> &WriterOptions {
        &self.options
    }

    /// Insert one batch's records into the `code.db` tables, atomically.
    #[expect(
        clippy::too_many_lines,
        reason = "one straight-line prepared-insert loop per table; splitting would scatter the 1:1 mapping"
    )]
    #[expect(
        clippy::cast_possible_wrap,
        reason = "u64 content_hash stored as its i64 bit pattern; round-trips losslessly"
    )]
    pub(crate) fn write_batch(&self, b: &WriteBatch) -> Result<(), DbError> {
        let tx = self.graph.unchecked_transaction().map_err(be)?;
        {
            let mut ins = tx
                .prepare_cached(
                    "INSERT INTO files(id,path,language,test,external,content_hash) \
                     VALUES(?,?,?,?,?,?)",
                )
                .map_err(be)?;
            for f in &b.files {
                ins.execute(params![
                    f.id,
                    f.path,
                    f.language.db_name(),
                    f.test,
                    f.external,
                    f.content_hash as i64, // bit-preserving; u64 doesn't fit SQLite INTEGER
                ])
                .map_err(be)?;
            }
        }
        {
            let mut ins = tx
                .prepare_cached(
                    "INSERT INTO packages(id,name,version,manager,external) VALUES(?,?,?,?,?)",
                )
                .map_err(be)?;
            for p in &b.packages {
                ins.execute(params![p.id, p.name, p.version, p.manager, p.external])
                    .map_err(be)?;
            }
        }
        {
            let mut ins = tx
                .prepare_cached(
                    "INSERT INTO symbols(id,pub_id,language,pkg_id,kind,name,name_lower,\
                     enclosing_sym_id,partial,nargs,targs,external,test) \
                     VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                )
                .map_err(be)?;
            for s in &b.symbols {
                // Hard invariant #1: every `pub_id` is a single shell-safe token
                // (it is handed to `kenn get <pub_id>` and MCP lookups). Rendering
                // is per-language in `kenn_indexer::pubid` (structural render →
                // `floor`); this is the net that makes a missed ingester fail
                // loudly rather than write a shell-hostile id. `is_safe` is the
                // shared contract both sides agree on.
                debug_assert!(
                    s.pub_id.chars().all(kenn_model::shell_safe::is_safe),
                    "ingester must render a shell-safe pub_id; got {:?} ({})",
                    s.pub_id,
                    s.language.db_name(),
                );
                // Hard invariant #2: the DB never stores a backtick in a name. The
                // *stripping* is per-language (each ingester unwraps its own
                // escaping — SCIP wrapping vs markdown code fences — since there
                // is no correct universal rule); this is only the net that makes a
                // missed ingester fail loudly instead of writing a bad name.
                debug_assert!(
                    !s.name.contains('`'),
                    "ingester must strip backticks per-language before insert; got {:?} ({})",
                    s.name,
                    s.language.db_name(),
                );
                ins.execute(params![
                    s.id,
                    s.pub_id,
                    s.language.db_name(),
                    s.pkg_id,
                    s.kind.db_name(),
                    s.name,
                    s.name.to_lowercase(),
                    s.enclosing_sym_id,
                    s.partial,
                    s.nargs,
                    s.targs,
                    s.external,
                    s.test,
                ])
                .map_err(be)?;
            }
        }
        {
            let mut ins = tx
                .prepare_cached("INSERT INTO symbol_docs(sym_id,sig,doc) VALUES(?,?,?)")
                .map_err(be)?;
            for d in &b.symbol_docs {
                ins.execute(params![d.sym_id, d.sig, d.doc]).map_err(be)?;
            }
        }
        {
            let mut ins = tx
                .prepare_cached("INSERT INTO file_docs(file_id,doc) VALUES(?,?)")
                .map_err(be)?;
            for d in &b.file_docs {
                ins.execute(params![d.file_id, d.doc]).map_err(be)?;
            }
        }
        {
            let mut ins = tx
                .prepare_cached(
                    "INSERT INTO defs(sym_id,file_id,start_line,start_col,end_line,end_col,\
                     body_start_line,body_end_line) \
                     VALUES(?,?,?,?,?,?,?,?)",
                )
                .map_err(be)?;
            for d in &b.defs {
                ins.execute(params![
                    d.sym_id,
                    d.file_id,
                    d.start_line,
                    d.start_col,
                    d.end_line,
                    d.end_col,
                    d.body_start_line,
                    d.body_end_line
                ])
                .map_err(be)?;
            }
        }
        {
            let mut ins = tx
                .prepare_cached(
                    "INSERT INTO edges(src_id,target_id,kind,field_op,import_kind,\
                     corr_source,corr_generator,corr_canon_id,link_grade,link_relation) \
                     VALUES(?,?,?,?,?,?,?,?,?,?)",
                )
                .map_err(be)?;
            for e in &b.edges {
                let kind = edge_kind_code(e.properties.kind());
                let (fo, ik, cs, cg, cc, lg, lr) = flatten_edge_properties(&e.properties);
                ins.execute(params![
                    e.src_id,
                    e.target_id,
                    kind,
                    fo,
                    ik,
                    cs,
                    cg,
                    cc,
                    lg,
                    lr
                ])
                .map_err(be)?;
            }
        }
        tx.commit().map_err(be)?;
        Ok(())
    }

    /// Read-only handles to the built DBs (for tests / next-task wiring).
    #[cfg(test)]
    pub(super) fn graph(&self) -> &Connection {
        &self.graph
    }
    #[cfg(test)]
    pub(super) fn knowledge(&self) -> &Connection {
        &self.knowledge
    }
}
