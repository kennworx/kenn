//! `list_tables` — the table axis of the graph, as a query.
//!
//! Every table the graph knows and every site that declares, modifies, or
//! accesses it, read STRAIGHT from the table edges of the published snapshot.
//! Both this and the atlas go through `kenn_indexer::atlas::tables`, so a table
//! can never mean one thing in the document and another at the prompt.
//!
//! The render caps are deliberately NOT applied here: a query must be able to
//! reach every table and every reference to one. Counts are honest totals, not
//! a truncated view that reads as complete.
//!
//! **Per-site, grouped by file.** A table's references have no package to roll
//! up into — a statement in a migration, an element in a changelog and a
//! function in application code are three files in three languages, and which
//! file made the reference is the answer to "what touches this, and where".

use kenn_indexer::atlas::tables::{self, RefKind, RefSite};
use kenn_model::{EdgeKind, Kind};
use kenn_store::api::Reader;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::QueryError;
use crate::types::ListResponse;

use crate::ctx::QueryCtx;
use crate::internal;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ListTablesArgs {
    /// Restrict to one table — its `pub_id` (`sql:orders`, `sql:public.orders`)
    /// or its display name. A name is a QUERY, not an identifier: two schemas
    /// can each hold an `events`, so when a name matches more than one table
    /// EVERY match is returned, each tagged with its own `pub_id`.
    #[serde(default)]
    pub table: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<crate::types::Pagination>,
}

/// One site that references a table — a resolvable handle to drill in.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TableRefView {
    /// The stable `pub_id` — feed it straight to `kenn get` / `kenn list`.
    pub id: String,
    pub name: String,
    /// The file the reference was made in.
    pub file: String,
    pub language: String,
    /// `declares`, `modifies`, or `accesses`.
    pub kind: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TableView {
    /// The table's own `pub_id` — its resolvable handle, and what you name to
    /// get its references. First column: it is the id a reader acts on.
    pub symbol: String,
    pub name: String,
    /// True when some statement in this workspace declares the table. False
    /// means the schema is owned elsewhere, which is ordinary rather than a
    /// defect — measured on a real repository, 85 of 133 tables were named only
    /// by an XML attribute.
    pub internal: bool,
    /// Distinct files referencing it, before any cap. The ranking key: breadth
    /// beats volume, because a table named by a migration, a changelog and
    /// application code is the architecturally interesting one.
    pub file_span: u64,
    /// Distinct languages referencing it, before any cap.
    pub language_span: u64,
    /// Total reference sites, before any cap.
    pub references: u64,
    /// The references themselves, grouped by file — populated only when a table
    /// was named, so the bare listing stays a flat scannable table.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<TableRefView>,
}

/// The edge kinds that make a table reference, and what each means.
///
/// A pair list rather than a fallible lookup: these three ARE the table edges,
/// so a `None` arm would be an unreachable branch that only costs complexity.
const TABLE_EDGES: [(EdgeKind, RefKind); 3] = [
    (EdgeKind::DefinesTable, RefKind::Declares),
    (EdgeKind::AltersTable, RefKind::Modifies),
    (EdgeKind::AccessesTable, RefKind::Accesses),
];

/// List the table axis, or one table's references.
///
/// # Errors
/// Returns a store error only when a read fails.
pub async fn list_tables(
    ctx: &QueryCtx<'_>,
    args: &ListTablesArgs,
) -> Result<ListResponse<TableView>, QueryError> {
    let want = args.table.as_deref();
    let symbols = ctx.read.scan_symbols().await.map_err(internal)?;
    let table_kind = Kind::SqlTable.db_name();

    let sites = gather_sites(ctx, &symbols).await?;

    let table_rows: Vec<(u32, String, String)> = symbols
        .iter()
        .filter(|s| s.kind == table_kind)
        .map(|s| (s.id, s.pub_id.clone(), s.name.clone()))
        .collect();

    // Borrowed views for the shared selection.
    let tables_in: Vec<(u32, &str)> = table_rows
        .iter()
        .map(|(id, _, name)| (*id, name.as_str()))
        .collect();
    let refs_in: Vec<(u32, RefSite<'_>)> = sites
        .iter()
        .map(|(t, s)| {
            (
                *t,
                RefSite {
                    symbol: s.symbol,
                    name: &s.name,
                    file: &s.file,
                    language: &s.language,
                    kind: s.kind,
                },
            )
        })
        .collect();

    let pub_id_of: std::collections::HashMap<u32, &str> = table_rows
        .iter()
        .map(|(id, pid, _)| (*id, pid.as_str()))
        .collect();
    let site_id_of: std::collections::HashMap<u32, &str> = sites
        .iter()
        .map(|(_, s)| (s.symbol, s.id.as_str()))
        .collect();

    let mut items: Vec<TableView> = Vec::new();
    for t in tables::select_tables(&tables_in, &refs_in) {
        let Some(pub_id) = pub_id_of.get(&t.node).copied() else {
            continue;
        };
        // A name argument is a QUERY: it matches the pub_id OR the display
        // name, and a name matching several keeps them all.
        if let Some(w) = want {
            if pub_id != w && t.name != w {
                continue;
            }
        }
        let mut views = Vec::new();
        if want.is_some() {
            for (file, group) in &t.by_file {
                for s in group {
                    views.push(TableRefView {
                        id: site_id_of
                            .get(&s.symbol)
                            .copied()
                            .unwrap_or_default()
                            .to_owned(),
                        name: s.name.to_owned(),
                        file: (*file).to_owned(),
                        language: s.language.to_owned(),
                        kind: s.kind.as_str().to_owned(),
                    });
                }
            }
        }
        items.push(TableView {
            symbol: pub_id.to_owned(),
            name: t.name.to_owned(),
            internal: t.internal,
            file_span: t.file_span,
            language_span: t.language_span,
            references: t.total_refs,
            sites: views,
        });
    }
    let (items, next) =
        crate::support::page_axis_items(items, args.pagination.as_ref(), ctx.snapshot_id)?;
    Ok(ListResponse { items, next })
}

/// Every table reference in the workspace, as owned rows.
///
/// Split out because it is the whole I/O half of the tool and has nothing to do
/// with ranking or presentation — and because bulk scans, not per-symbol
/// fetches, are what make it viable: the reference set is every table edge in
/// the workspace, and a round-trip each is the shape that made the code→table
/// pass unviable before it was rewritten.
async fn gather_sites(
    ctx: &QueryCtx<'_>,
    symbols: &[kenn_store::SymbolRow],
) -> Result<Vec<(u32, RefSiteOwned)>, QueryError> {
    // Per-site edges, not the rolled-up aggregate ones the contracts
    // axis reads: which FILE made the reference is the answer here, and
    // an aggregate has already collapsed that.
    //
    // Bulk scans, not per-symbol fetches: the reference set is every
    // table edge in the workspace, and a round-trip each would be the
    // shape that made the code→table pass unviable.
    let files = ctx.read.scan_files().await.map_err(internal)?;
    let path_of: std::collections::HashMap<u32, &str> =
        files.iter().map(|f| (f.id, f.path.as_str())).collect();
    let file_of: std::collections::HashMap<u32, u32> = ctx
        .read
        .scan_def_files()
        .await
        .map_err(internal)?
        .into_iter()
        .collect();
    let sym_of: std::collections::HashMap<u32, &kenn_store::SymbolRow> =
        symbols.iter().map(|s| (s.id, s)).collect();

    let mut sites: Vec<(u32, RefSiteOwned)> = Vec::new();
    for (kind, rk) in TABLE_EDGES {
        for (src, target) in ctx
            .read
            .scan_edges(kind.db_name())
            .await
            .map_err(internal)?
        {
            let Some(s) = sym_of.get(&src) else { continue };
            let file = file_of
                .get(&src)
                .and_then(|fid| path_of.get(fid))
                .copied()
                .unwrap_or("")
                .to_owned();
            sites.push((
                target,
                RefSiteOwned {
                    symbol: src,
                    id: s.pub_id.clone(),
                    name: s.name.clone(),
                    file,
                    language: s.language.clone(),
                    kind: rk,
                },
            ));
        }
    }
    Ok(sites)
}

/// Owned mirror of [`RefSite`], because the borrowed form cannot outlive the
/// store rows it is projected from.
struct RefSiteOwned {
    symbol: u32,
    id: String,
    name: String,
    file: String,
    language: String,
    kind: RefKind,
}
