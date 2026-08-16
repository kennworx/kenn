//! The XML→table barrier step: store I/O around the pure resolution core.
//!
//! Runs after the producers have joined, because both inputs are things
//! producers wrote: the XML element nodes with their two surfaces, and the
//! table nodes the `.sql` pass emitted. Neither producer carries pending state
//! for it — this reads the building store, which is what keeps both
//! barrier-free. Everything it decides lives in [`resolve`](super::resolve).

use kenn_model::Language;
use kenn_store::api::Reader;
use kenn_store::DbReader;
use tokio::runtime::Handle;

use super::resolve::{resolve, XmlSqlCounts};
use crate::sink::BatchSink;
use crate::sql::mint::TableMinter;
use crate::sql::registry::known_tables;

/// Emit the table references the workspace's XML makes.
///
/// `minter` is threaded in rather than built here: more than one barrier step
/// mints into the `Sql` partition, and two allocators built independently from
/// the same high-water mark hand two tables one id.
///
/// # Errors
/// Returns a store error only when a read or write fails.
pub fn ingest_xml_tables(
    reader: &DbReader,
    handle: &Handle,
    config: &kenn_config::XmlSqlConfig,
    minter: &mut TableMinter,
    mut sink: BatchSink,
) -> Result<XmlSqlCounts, kenn_store::DbError> {
    let elements = handle.block_on(reader.scan_symbol_surfaces(Language::Xml.db_name()))?;
    if elements.is_empty() {
        sink.finish()?;
        return Ok(XmlSqlCounts::default());
    }
    let symbols = handle.block_on(reader.scan_symbols())?;
    let (known, existing) = known_tables(&symbols);

    let found = resolve(&known, config, &elements);

    let mut ids = existing;
    crate::sql::emit::mint_tables(&mut sink, minter, &found.minted, &mut ids)?;
    let dropped = crate::sql::emit::emit_table_edges(
        &mut sink,
        &ids,
        // The element, never its document: a table's references should point at
        // the `<createTable>` that named it, not at the file.
        found
            .refs
            .iter()
            .map(|r| (r.sym_id, &r.table, r.role, r.grade)),
    )?;

    sink.finish()?;
    let mut counts = found.counts;
    counts.refs_dropped = dropped;
    Ok(counts)
}
