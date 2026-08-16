//! Writing table nodes and table edges — shared by every step that resolves
//! references to tables.
//!
//! Two bridges reach tables today, from code literals and from XML, and a third
//! is foreseeable. They differ entirely in how they *find* a reference and not
//! at all in what they then write: mint the identities nothing declares, then
//! emit one edge per reference. Duplicating that produced two copies of the
//! role→edge-kind mapping within an hour of each other, which is precisely the
//! drift the shared-registry rule was written against — a table's references
//! would split into two vocabularies depending on which surface carried them.

use std::collections::BTreeMap;

use kenn_model::{
    EdgeKind, EdgeProperties, EdgeRecord, Kind, Language, LinkGrade, ShortId, SymbolRecord,
};

use super::mint::TableMinter;
use super::parse::RefRole;
use super::registry::TableKey;
use crate::sink::BatchSink;

/// The edge kind a reference role produces.
///
/// One mapping for every producer and bridge. Two copies would let a table's
/// references arrive under different relations depending on what named it, so
/// `list usages` would answer differently for the same fact.
#[must_use]
pub const fn edge_kind(role: RefRole) -> EdgeKind {
    match role {
        RefRole::Defines => EdgeKind::DefinesTable,
        RefRole::Alters => EdgeKind::AltersTable,
        RefRole::Accesses => EdgeKind::AccessesTable,
    }
}

/// Write a node for each identity nothing in the workspace declares, and add it
/// to `ids` so the edges that follow can reach it.
///
/// A table exists in its database whether or not this workspace declares it,
/// and a workspace whose schema is owned elsewhere — or named only by
/// attributes — is the case these steps most need to serve.
///
/// # Errors
/// Propagates a store write failure.
pub fn mint_tables(
    sink: &mut BatchSink,
    minter: &mut TableMinter,
    keys: &[TableKey],
    ids: &mut BTreeMap<TableKey, ShortId>,
) -> Result<(), kenn_store::DbError> {
    for key in keys {
        let id = minter.mint();
        sink.push_symbol(SymbolRecord {
            id,
            // Floored like every other producer's id. The identity filter in
            // `sql::parse` keeps a variable out of the graph; this is the net
            // under it, so a name that slips past cannot write a shell-hostile
            // id and abort the run.
            pub_id: crate::pubid::floor(
                kenn_model::id::sql::table_id(key.schema.as_deref(), &key.name).as_str(),
            ),
            language: Language::Sql,
            pkg_id: 0,
            kind: Kind::SqlTable,
            name: key.name.clone(),
            enclosing_sym_id: 0,
            partial: false,
            nargs: 0,
            targs: 0,
            external: true,
            test: false,
        })?;
        ids.insert(key.clone(), id);
    }
    Ok(())
}

/// Emit one edge per reference, from the symbol that carried it.
///
/// Returns how many references were dropped for want of a target node.
///
/// A reference whose table is absent from `ids` is skipped rather than
/// erroring: minting runs first, so absence means the identity resolved to
/// something no node exists for, and a missing edge is a smaller wrong than a
/// failed run.
///
/// **The count is the point.** Skipping is still the right recovery, but a
/// silent one hid a real defect through a full corpus run, every unit test, and
/// a green gate: the mint guard tested a table's bare name while the reference
/// carried its whole key, so one spelling satisfied the guard for another and
/// the loser's edge vanished here. It cost a `createTable` declaration on a real
/// workspace and took a two-index hand diff to find. A non-zero count is now a
/// bug report rather than an absence nobody can see.
///
/// # Errors
/// Propagates a store write failure.
pub fn emit_table_edges<'a>(
    sink: &mut BatchSink,
    ids: &BTreeMap<TableKey, ShortId>,
    refs: impl Iterator<Item = (ShortId, &'a TableKey, RefRole, LinkGrade)>,
) -> Result<u64, kenn_store::DbError> {
    let mut dropped = 0u64;
    for (src_id, table, role, grade) in refs {
        let Some(target_id) = ids.get(table).copied() else {
            dropped += 1;
            continue;
        };
        sink.push_edge(EdgeRecord {
            src_id,
            target_id,
            properties: EdgeProperties::Table {
                kind: edge_kind(role),
                grade,
            },
        })?;
    }
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_role_maps_to_its_own_edge_kind() {
        // Pinned in one place, because two copies of this mapping is exactly
        // what this module exists to prevent.
        assert_eq!(edge_kind(RefRole::Defines), EdgeKind::DefinesTable);
        assert_eq!(edge_kind(RefRole::Alters), EdgeKind::AltersTable);
        assert_eq!(edge_kind(RefRole::Accesses), EdgeKind::AccessesTable);
    }

    /// A reference with no target node is counted, not just skipped.
    ///
    /// The skip itself is right — one missing edge should not fail an index —
    /// but a *silent* skip hid a lost `createTable` declaration through a full
    /// corpus run, every unit test, and a green gate, and was found only by
    /// diffing two indexes by hand. The count is what makes the next occurrence
    /// a one-line report.
    ///
    /// Added on review: the counter shipped with its zero case verified on a
    /// corpus and its non-zero case verified nowhere, so deleting the
    /// `dropped += 1` would have failed nothing.
    #[test]
    fn a_reference_with_no_target_node_is_counted() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let dir = tempfile::TempDir::new().expect("tempdir");
        let writer = rt
            .block_on(kenn_store::open_writer(
                dir.path(),
                kenn_store::WriterOptions::default(),
            ))
            .expect("open_writer");
        let mut sink = crate::sink::BatchSink::new(writer, rt.handle().clone(), 16);

        let present = TableKey::new(None, "orders".into());
        let absent = TableKey::new(Some("archive".into()), "orders".into());
        let mut ids = BTreeMap::new();
        ids.insert(present.clone(), 1u32);

        let refs = [
            (10u32, &present, RefRole::Defines, LinkGrade::Exact),
            (11u32, &absent, RefRole::Accesses, LinkGrade::Exact),
            (12u32, &absent, RefRole::Accesses, LinkGrade::Exact),
        ];
        let dropped =
            emit_table_edges(&mut sink, &ids, refs.into_iter()).expect("emit_table_edges");

        assert_eq!(
            dropped, 2,
            "both references to the unminted identity are counted"
        );
    }
}
