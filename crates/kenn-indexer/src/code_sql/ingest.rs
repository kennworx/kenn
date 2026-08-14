//! The code→table barrier step: store I/O around the pure resolution core.
//!
//! Runs after phase 1 has joined, because both inputs are things producers
//! wrote: the code symbols with their body extents, and the table nodes the
//! `.sql` pass emitted. Everything it decides lives in
//! [`resolve`](super::resolve); this reads, writes, and counts.

use std::path::Path;

use kenn_store::api::Reader;
use kenn_store::DbReader;
use tokio::runtime::Handle;

use super::resolve::{resolve, CodeSqlCounts};
use crate::sink::BatchSink;
use crate::sql::mint::TableMinter;

/// Emit the table references the workspace's own source makes.
///
/// `minter` is threaded in rather than built here: more than one barrier step
/// mints into the `Sql` partition, and two allocators built independently from
/// the same high-water mark hand two tables one id.
///
/// # Errors
/// Returns a store error only when a read or write fails. An unreadable source
/// file contributes nothing and is not an error — the file may have been
/// deleted between indexing and this step.
pub fn ingest_code_tables(
    reader: &DbReader,
    handle: &Handle,
    workspace_root: &Path,
    minter: &mut TableMinter,
    mut sink: BatchSink,
) -> Result<CodeSqlCounts, kenn_store::DbError> {
    let symbols = handle.block_on(reader.scan_symbols())?;
    let bodies = handle.block_on(reader.scan_symbol_bodies())?;

    let (known, existing) = crate::sql::registry::known_tables(&symbols);

    let found = resolve(&known, &bodies, &|path| {
        std::fs::read_to_string(workspace_root.join(path)).ok()
    });

    let mut ids = existing;
    crate::sql::emit::mint_tables(&mut sink, minter, &found.minted, &mut ids)?;
    crate::sql::emit::emit_table_edges(
        &mut sink,
        &ids,
        found
            .refs
            .iter()
            .map(|r| (r.sym_id, &r.table, r.role, r.grade)),
    )?;

    sink.finish()?;
    Ok(found.counts)
}
