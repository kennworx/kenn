//! End-of-run roll-up: per-symbol graph → weighted undirected aggregated
//! graph, persisted to the snapshot's `aggregate_nodes` /
//! `aggregate_edges` tables.
//!
//! Why this lives in the indexer rather than `kenn-analyze`: every
//! snapshot ships the rolled-up graph as a first-class artifact. Analysis
//! tooling reads the tables directly; the prototype's recompute path
//! stays as a fallback for snapshots that pre-date this step.
//!
//! The roll-up rules and per-kind weights are spec-frozen and worth
//! restating here because the spec is the only place that documents
//! WHY (see `openspec/changes/.../specs/graph-analysis/spec.md`):
//!
//! - **Why class-like → module-like → self**: a method's natural owner is
//!   its class, but a free function lives at module scope; both rounds
//!   are captured by walking `enclosing_symbol` and preferring the
//!   nearest class-like over the nearest module-like. A symbol with no
//!   enclosing scope stays its own aggregate.
//! - **Why per-kind separate edges**: a `calls` link and a `type_use`
//!   link between the same two aggregates are different evidence of
//!   coupling; collapsing them into one weight hides that. Downstream
//!   consumers can re-weight without re-ingest.
//! - **Why these weights**: tuned to favor semantic coupling. Calls (3)
//!   beat type/field usage (2) beat structural overrides/imports (1).
//!   The numbers come from prototype validation across three real
//!   repos; changing them is a separate design decision.

use std::collections::{HashMap, HashSet};

use kenn_model::{
    AggregateEdgeRecord, AggregateNodeRecord, EdgeKind, FileRecord, Kind, PackageRecord, ShortId,
    SymbolRecord,
};

use crate::package_layout::PackageLayout;

/// Edge kinds kept in the aggregated graph and their per-edge weights.
///
/// Code coupling (`calls` … `imports`) plus markdown link coupling
/// (`links_to`, `embeds`) — the latter so a markdown vault produces a
/// note-to-note graph (an `embeds`/transclusion is tighter coupling than
/// a plain reference, hence weight 2 vs 1, mirroring calls > type-use).
///
/// Skipped kinds (`defined_in`, `contains`, `generic_constraint`,
/// `corresponds_to`) are the structural / equivalence relations that
/// just re-derive the symbol tree. `links_to_file` is also skipped: its
/// target is a file row, not a symbol, so it has no aggregate node.
pub const KEPT_EDGE_KINDS: &[(EdgeKind, u32)] = &[
    (EdgeKind::Calls, 3),
    (EdgeKind::TypeUse, 2),
    (EdgeKind::FieldAccess, 2),
    (EdgeKind::Implements, 2),
    (EdgeKind::Instantiates, 2),
    (EdgeKind::Overrides, 1),
    (EdgeKind::Imports, 1),
    (EdgeKind::Embeds, 2),
    (EdgeKind::LinksTo, 1),
];

const UNANCHORED: &str = "<unanchored>";

#[inline]
const fn is_module_like(kind: Kind) -> bool {
    matches!(kind, Kind::Module | Kind::Namespace | Kind::Package)
}

/// Kinds that terminate the roll-up walk as their own aggregate: the
/// nominal code types (class-like) plus a markdown `Document` or
/// `Attachment`. A note is the natural leaf of the markdown graph — its
/// sections roll up into it, but the document itself must NOT roll up into
/// its directory module, or every note would collapse to its folder and
/// the note-to-note graph would be lost. An attachment is always a leaf.
#[inline]
const fn is_aggregate_leaf(kind: Kind) -> bool {
    kind.is_class_like() || matches!(kind, Kind::Document | Kind::Attachment)
}

/// For each symbol, compute its aggregate `short_id` — the nearest
/// enclosing leaf (class-like or markdown document), else the nearest
/// enclosing module-like, else itself. Cycle-safe: terminates if
/// `enclosing_symbol` chains revisit a node.
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "in-tree callers pass the std-default HashMap; generalizing over BuildHasher only adds noise"
)]
pub fn compute_aggregate_ids(
    symbols: &HashMap<ShortId, SymbolRecord>,
) -> HashMap<ShortId, ShortId> {
    let mut out = HashMap::with_capacity(symbols.len());
    for &sid in symbols.keys() {
        out.insert(sid, walk_to_aggregate(sid, symbols));
    }
    out
}

fn walk_to_aggregate(start: ShortId, symbols: &HashMap<ShortId, SymbolRecord>) -> ShortId {
    let mut seen = HashSet::new();
    let mut cur = start;
    let mut module_fallback: Option<ShortId> = None;
    loop {
        if !seen.insert(cur) {
            return module_fallback.unwrap_or(start);
        }
        let Some(row) = symbols.get(&cur) else {
            return module_fallback.unwrap_or(start);
        };
        if is_aggregate_leaf(row.kind) {
            return cur;
        }
        // An *external* module must never be an aggregate root: some producers
        // (scip-python) encode the module path in a member's descriptor without
        // ever emitting the module as a defined symbol, so it drains as a
        // def-less `external` stub. Rolling first-party functions into it would
        // collapse them onto an `<unanchored>` node — losing their internal
        // edges and dropping the package to a bare "document". Only internal
        // modules anchor their members.
        if is_module_like(row.kind) && !row.external && module_fallback.is_none() {
            module_fallback = Some(cur);
        }
        if row.enclosing_sym_id == 0 || row.enclosing_sym_id == cur {
            return module_fallback.unwrap_or(start);
        }
        cur = row.enclosing_sym_id;
    }
}

/// Aggregate per-symbol edges of one kind into weighted undirected
/// aggregate-pair edges. Skips self-loops on the aggregate graph,
/// applies the module-to-module-only rule for `imports`, and dedupes
/// undirected by sorting endpoints.
///
/// Returns a map `(min_agg, max_agg, kind) -> total_weight`. Callers
/// typically iterate `KEPT_EDGE_KINDS`, call this per kind, then merge
/// into a single sorted edge list for persistence.
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "in-tree callers pass the std-default HashMap; generalizing over BuildHasher only adds noise"
)]
pub fn aggregate_edges_by_kind(
    aggregate_of: &HashMap<ShortId, ShortId>,
    aggregate_kind: &HashMap<ShortId, Kind>,
    edges: &[(ShortId, ShortId)],
    kind: EdgeKind,
    weight: u32,
) -> HashMap<(ShortId, ShortId, EdgeKind), u32> {
    let mut out: HashMap<(ShortId, ShortId, EdgeKind), u32> = HashMap::new();
    let imports_only_modules = matches!(kind, EdgeKind::Imports);
    for &(src, tgt) in edges {
        let Some(&src_agg) = aggregate_of.get(&src) else {
            continue;
        };
        let Some(&tgt_agg) = aggregate_of.get(&tgt) else {
            continue;
        };
        if src_agg == tgt_agg {
            continue;
        }
        if imports_only_modules {
            let src_ok = aggregate_kind
                .get(&src_agg)
                .copied()
                .is_some_and(is_module_like);
            let tgt_ok = aggregate_kind
                .get(&tgt_agg)
                .copied()
                .is_some_and(is_module_like);
            if !(src_ok && tgt_ok) {
                continue;
            }
        }
        // Directed: keep src → tgt. The analysis normalizes to undirected on load
        // (Louvain / weighted-degree); consumers that want direction — the atlas
        // `Depends on` links — read it as-is.
        *out.entry((src_agg, tgt_agg, kind)).or_insert(0) += weight;
    }
    out
}

/// Resolve an anchor (id + human-readable name) for every aggregate.
///
/// Priority chain (each tier wins over the next):
///   1. `symbols[agg].pkg` when non-zero — anchor name = package name
///   2. Longest-matching package marker covering the symbol's primary
///      def file (`Cargo.toml`, `package.json`, `*.csproj`, `go.mod`,
///      `pyproject.toml`) — anchor name = the declared package name.
///      Makes monorepo layouts (`crates/foo/src/lib.rs`) anchor at
///      `foo` instead of the bare first segment `crates`.
///   3. First `/`-separated segment of the symbol's primary def file
///   4. Literal `<unanchored>` when none of the above apply
///
/// Anchors are interned to small `u32` ids in the returned table.
/// Determinism: interning order is anchor-name ascending — the same
/// inputs always produce the same id assignment across runs.
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "in-tree callers pass the std-default HashMap/HashSet; generalizing over BuildHasher only adds noise"
)]
pub fn resolve_anchors(
    aggregate_ids: &HashSet<ShortId>,
    symbols: &HashMap<ShortId, SymbolRecord>,
    files: &HashMap<ShortId, String>,
    packages: &HashMap<ShortId, String>,
    primary_def_file: &HashMap<ShortId, ShortId>,
    layout: &PackageLayout,
) -> HashMap<ShortId, (u32, String)> {
    // First resolve the *name* per aggregate, then intern names in sorted
    // order so id assignment is deterministic.
    let mut name_for: HashMap<ShortId, String> = HashMap::with_capacity(aggregate_ids.len());
    for &agg in aggregate_ids {
        let name = anchor_name_for(agg, symbols, files, packages, primary_def_file, layout);
        name_for.insert(agg, name);
    }
    let mut sorted_unique: Vec<String> = name_for.values().cloned().collect();
    sorted_unique.sort();
    sorted_unique.dedup();
    let mut intern: HashMap<String, u32> = HashMap::with_capacity(sorted_unique.len());
    for (idx, n) in sorted_unique.into_iter().enumerate() {
        // 0 is reserved as a "no-anchor" sentinel by convention; start at 1.
        // u32 ids fit any conceivable anchor count.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "anchor count vastly under u32::MAX in any real workspace"
        )]
        let id = (idx as u32) + 1;
        intern.insert(n, id);
    }
    let mut out: HashMap<ShortId, (u32, String)> = HashMap::with_capacity(aggregate_ids.len());
    for (agg, name) in name_for {
        let id = intern.get(&name).copied().unwrap_or(0);
        out.insert(agg, (id, name));
    }
    out
}

fn anchor_name_for(
    agg: ShortId,
    symbols: &HashMap<ShortId, SymbolRecord>,
    files: &HashMap<ShortId, String>,
    packages: &HashMap<ShortId, String>,
    primary_def_file: &HashMap<ShortId, ShortId>,
    layout: &PackageLayout,
) -> String {
    if let Some(s) = symbols.get(&agg) {
        if s.pkg_id != 0 {
            if let Some(p) = packages.get(&s.pkg_id) {
                if !p.is_empty() {
                    return p.clone();
                }
            }
        }
    }
    if let Some(&file_id) = primary_def_file.get(&agg) {
        if let Some(path) = files.get(&file_id) {
            if let Some(name) = layout.anchor_for(path) {
                return name.to_string();
            }
            if let Some(seg) = first_path_segment(path) {
                return seg.to_string();
            }
        }
    }
    UNANCHORED.to_string()
}

fn first_path_segment(path: &str) -> Option<&str> {
    path.split('/').find(|s| !s.is_empty())
}

/// Build the `AggregateNodeRecord` list — one per anchor that actually
/// participated in an aggregated edge (orphan symbols don't get rows).
/// `participating` is the set of `(min_agg, max_agg)` endpoints from
/// `aggregate_edges_by_kind` results, flattened.
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "in-tree callers pass the std-default HashMap/HashSet; generalizing over BuildHasher only adds noise"
)]
pub fn build_aggregate_nodes(
    participating: &HashSet<ShortId>,
    symbols: &HashMap<ShortId, SymbolRecord>,
    anchors: &HashMap<ShortId, (u32, String)>,
) -> Vec<AggregateNodeRecord> {
    let mut out: Vec<AggregateNodeRecord> = participating
        .iter()
        .filter_map(|sid| {
            let s = symbols.get(sid)?;
            let (anchor_id, anchor_name) = anchors
                .get(sid)
                .cloned()
                .unwrap_or((0, UNANCHORED.to_string()));
            Some(AggregateNodeRecord {
                id: s.id,
                kind: s.kind,
                name: s.name.clone(),
                language: s.language,
                external: s.external,
                test: s.test,
                anchor_id,
                anchor_name,
            })
        })
        .collect();
    // Deterministic on-disk layout: sort by short_id ascending.
    out.sort_by_key(|n| n.id);
    out
}

/// Flatten the per-kind accumulated edge maps into a single sorted
/// `Vec<AggregateEdgeRecord>` ready for persistence. Sort order is
/// `(src_id, dst_id, kind_discriminant)` so the on-disk byte layout is
/// stable across runs.
#[must_use]
pub fn flatten_edges<I>(per_kind: I) -> Vec<AggregateEdgeRecord>
where
    I: IntoIterator<Item = HashMap<(ShortId, ShortId, EdgeKind), u32>>,
{
    let mut out: Vec<AggregateEdgeRecord> = Vec::new();
    for m in per_kind {
        for ((src_id, dst_id, kind), weight) in m {
            out.push(AggregateEdgeRecord {
                src_id,
                dst_id,
                kind,
                weight,
            });
        }
    }
    out.sort_by(|a, b| {
        a.src_id
            .cmp(&b.src_id)
            .then(a.dst_id.cmp(&b.dst_id))
            .then((a.kind as u32).cmp(&(b.kind as u32)))
    });
    out
}

/// One-shot driver: pulls everything the aggregation step needs from a
/// writer that has already flushed its per-unit data, computes the
/// rolled-up nodes + edges, and persists them via the writer's
/// aggregate-table methods. Returns `(node_count, edge_count)`.
///
/// `Ok(None)` signals there is nothing to aggregate (no symbols); the
/// pipeline treats this as "skip".
#[must_use = "result reports skipped vs ran"]
#[expect(
    clippy::too_many_lines,
    reason = "linear aggregate → persist → hook → atlas sequence; the shared writer/maps make splitting scatter state"
)]
pub async fn compute_and_persist(
    writer: &kenn_store::DbWriter,
    layout: &PackageLayout,
    aggregated_hook: crate::pipeline::PostAggregateHook,
    atlas: Option<&crate::atlas::producer::AtlasContext>,
) -> Result<Option<(usize, usize)>, kenn_store::api::DbError> {
    let symbol_vec = writer.scan_symbols_for_aggregation().await?;
    if symbol_vec.is_empty() {
        return Ok(None);
    }
    let symbols: HashMap<ShortId, SymbolRecord> =
        symbol_vec.into_iter().map(|s| (s.id, s)).collect();

    let files_vec: Vec<FileRecord> = writer.scan_files_for_aggregation().await?;
    let files: HashMap<ShortId, String> = files_vec.into_iter().map(|f| (f.id, f.path)).collect();
    let packages_vec: Vec<PackageRecord> = writer.scan_packages_for_aggregation().await?;
    let packages: HashMap<ShortId, String> =
        packages_vec.into_iter().map(|p| (p.id, p.name)).collect();
    let def_files = writer.scan_def_files_for_aggregation().await?;
    // primary def file = first (min-file-id) seen per symbol; the defs
    // table is keyed by (sym, file, line) so the iteration order already
    // groups by sym, but we still take the smallest file_id for
    // determinism in the face of partial-class multi-file declarations.
    let mut primary_def_file: HashMap<ShortId, ShortId> = HashMap::new();
    for (sym, file) in def_files {
        primary_def_file
            .entry(sym)
            .and_modify(|f| {
                if file < *f {
                    *f = file;
                }
            })
            .or_insert(file);
    }

    let aggregate_of = compute_aggregate_ids(&symbols);
    let aggregate_kind: HashMap<ShortId, Kind> = aggregate_of
        .values()
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .filter_map(|agg| symbols.get(&agg).map(|a| (agg, a.kind)))
        .collect();

    let mut per_kind_maps: Vec<HashMap<(ShortId, ShortId, EdgeKind), u32>> = Vec::new();
    for &(kind, weight) in KEPT_EDGE_KINDS {
        let edges = writer.scan_edges_for_aggregation(kind).await?;
        if edges.is_empty() {
            continue;
        }
        let m = aggregate_edges_by_kind(&aggregate_of, &aggregate_kind, &edges, kind, weight);
        if !m.is_empty() {
            per_kind_maps.push(m);
        }
    }

    let mut participating: HashSet<ShortId> = HashSet::new();
    for m in &per_kind_maps {
        for &(a, b, _) in m.keys() {
            participating.insert(a);
            participating.insert(b);
        }
    }

    let anchors = resolve_anchors(
        &participating,
        &symbols,
        &files,
        &packages,
        &primary_def_file,
        layout,
    );
    let nodes = build_aggregate_nodes(&participating, &symbols, &anchors);
    let edges = flatten_edges(per_kind_maps);
    let counts = (nodes.len(), edges.len());
    writer.write_aggregate_tables(&nodes, &edges).await?;

    // The analysis hook runs FIRST now: it computes + persists the flat-Louvain
    // communities (`analysis_node_membership` / `analysis_flat_communities`) that
    // the atlas reads back for its `domains` axis. It takes the records by value,
    // so we clone when the atlas needs the originals for its package axis; with no
    // atlas we hand the originals straight over.
    if let Some(ctx) = atlas {
        aggregated_hook(nodes.clone(), edges.clone(), writer.clone()).await?;

        // Atlas (`atlas` capability): resolve anchors for every symbol (not just
        // the edge-participating ones), then build + write the OKF bundle from the
        // same aggregate graph. Runs on the shared pipeline (CLI + MCP).
        let all_agg: HashSet<ShortId> = aggregate_of.values().copied().collect();
        let atlas_anchors = resolve_anchors(
            &all_agg,
            &symbols,
            &files,
            &packages,
            &primary_def_file,
            layout,
        );
        // Per-symbol source range at its primary (min-file) def — the row with the
        // smallest start line — for central symbols (`start:end`).
        let mut primary_def_range: HashMap<ShortId, (u32, u32)> = HashMap::new();
        for (sym, file, start, end) in writer.scan_def_lines_for_aggregation().await? {
            if primary_def_file.get(&sym) == Some(&file) {
                primary_def_range
                    .entry(sym)
                    .and_modify(|r| {
                        if start < r.0 {
                            *r = (start, end);
                        }
                    })
                    .or_insert((start, end));
            }
        }
        // The domains axis: the flat communities the hook just wrote, read back on
        // the writer's own connection (atlas ⊥ kenn-analyze — it consumes the
        // persisted tables, never recomputes clustering).
        let membership = writer.scan_analysis_node_membership().await?;
        let flat = writer.scan_analysis_flat_communities().await?;
        // File-level module docs, to seed each package concept's `description`
        // (atlas tasks 3.4/8.4) verbatim from its root module.
        let file_docs: HashMap<ShortId, String> =
            writer.scan_file_docs().await?.into_iter().collect();
        let (mut concepts, domains, shape) = crate::atlas::producer::build_concepts(
            &symbols,
            &files,
            &primary_def_file,
            &aggregate_of,
            &atlas_anchors,
            &nodes,
            &edges,
            &membership,
            &flat,
            &primary_def_range,
            &file_docs,
            &crate::atlas::producer::ShapeMeta {
                workspace_name: &ctx.workspace_name,
                freshness: &ctx.freshness,
                timestamp: &ctx.timestamp,
            },
        );
        // The atlas respects .gitignore: drop any concept whose dir is ignored.
        concepts.retain(|c| !dir_is_gitignored(&ctx.source_root, &c.resource));
        crate::atlas::producer::write_bundle(&ctx.out_dir, &shape, &concepts, &domains)
            .map_err(|e| kenn_store::api::DbError::Backend(format!("atlas write: {e}")))?;
    } else {
        aggregated_hook(nodes, edges, writer.clone()).await?;
    }
    Ok(Some(counts))
}

/// Whether `git` considers `rel` (a workspace-relative dir) ignored under
/// `root`. Best-effort — a missing/erroring `git` treats nothing as ignored.
fn dir_is_gitignored(root: &std::path::Path, rel: &str) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "-q", rel])
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenn_model::Language;

    fn sym(id: ShortId, kind: Kind, enclosing: ShortId) -> SymbolRecord {
        SymbolRecord {
            id,
            pub_id: format!("test:s{id}"),
            language: Language::Rust,
            pkg_id: 0,
            kind,
            name: format!("s{id}"),
            enclosing_sym_id: enclosing,
            partial: false,
            nargs: 0,
            targs: 0,
            external: false,
            test: false,
        }
    }

    fn map_of(rows: Vec<SymbolRecord>) -> HashMap<ShortId, SymbolRecord> {
        rows.into_iter().map(|s| (s.id, s)).collect()
    }

    // ── compute_aggregate_ids ──────────────────────────────────────

    #[test]
    fn method_rolls_up_to_class() {
        let m = map_of(vec![
            sym(1, Kind::Module, 0),
            sym(2, Kind::Class, 1),
            sym(3, Kind::Method, 2),
        ]);
        let agg = compute_aggregate_ids(&m);
        assert_eq!(agg[&3], 2);
        assert_eq!(agg[&2], 2);
        assert_eq!(agg[&1], 1);
    }

    #[test]
    fn free_function_rolls_up_to_module() {
        let m = map_of(vec![sym(1, Kind::Module, 0), sym(2, Kind::Function, 1)]);
        let agg = compute_aggregate_ids(&m);
        assert_eq!(agg[&2], 1);
    }

    #[test]
    fn free_function_under_external_module_stays_self() {
        // scip-python encodes the module path in a member's descriptor but
        // never emits the module as a defined symbol, so it drains as a
        // def-less `external` stub. A first-party function enclosed by such an
        // external module must NOT roll into it (that collapses it onto an
        // `<unanchored>` node and drops the package to a "document") — it
        // stays its own aggregate, exactly like a crate-root Rust fn.
        let ext_module = SymbolRecord {
            external: true,
            ..sym(1, Kind::Namespace, 0)
        };
        let m = map_of(vec![ext_module, sym(2, Kind::Function, 1)]);
        let agg = compute_aggregate_ids(&m);
        assert_eq!(
            agg[&2], 2,
            "internal fn must not roll into an external module"
        );
    }

    #[test]
    fn field_rolls_up_to_class() {
        let m = map_of(vec![
            sym(1, Kind::Namespace, 0),
            sym(2, Kind::Class, 1),
            sym(3, Kind::Field, 2),
        ]);
        let agg = compute_aggregate_ids(&m);
        assert_eq!(agg[&3], 2);
    }

    #[test]
    fn orphan_stays_self() {
        let m = map_of(vec![sym(1, Kind::Function, 0)]);
        let agg = compute_aggregate_ids(&m);
        assert_eq!(agg[&1], 1);
    }

    #[test]
    fn markdown_section_rolls_up_to_document_not_folder() {
        // dir module(1) ⊃ document(2) ⊃ section(3) ⊃ subsection(4).
        // Sections roll up to their note (document), and the note is its
        // own aggregate — it must NOT collapse into the folder module.
        let m = map_of(vec![
            sym(1, Kind::Module, 0),
            sym(2, Kind::Document, 1),
            sym(3, Kind::Section, 2),
            sym(4, Kind::Section, 3),
        ]);
        let agg = compute_aggregate_ids(&m);
        assert_eq!(agg[&2], 2); // document is its own aggregate (leaf)
        assert_eq!(agg[&3], 2); // section → document
        assert_eq!(agg[&4], 2); // subsection → document
        assert_eq!(agg[&1], 1); // folder module stays itself
    }

    #[test]
    fn cycle_terminates_safely() {
        let mut m = map_of(vec![sym(1, Kind::Function, 2), sym(2, Kind::Function, 1)]);
        m.get_mut(&1).unwrap().enclosing_sym_id = 2;
        m.get_mut(&2).unwrap().enclosing_sym_id = 1;
        let agg = compute_aggregate_ids(&m);
        assert_eq!(agg[&1], 1);
        assert_eq!(agg[&2], 2);
    }

    // ── aggregate_edges_by_kind ────────────────────────────────────

    fn kind_map(items: &[(ShortId, Kind)]) -> HashMap<ShortId, Kind> {
        items.iter().copied().collect()
    }

    #[test]
    fn weighted_accumulation_across_multiple_edges() {
        // Two method-to-method calls between class 2 and class 5 → one
        // aggregate edge with weight 3 + 3 = 6.
        let agg: HashMap<ShortId, ShortId> = [(10, 2), (11, 2), (20, 5), (21, 5)].into();
        let kinds = kind_map(&[(2, Kind::Class), (5, Kind::Class)]);
        let edges = vec![(10, 20), (11, 21)];
        let out = aggregate_edges_by_kind(&agg, &kinds, &edges, EdgeKind::Calls, 3);
        assert_eq!(out.len(), 1);
        assert_eq!(out[&(2, 5, EdgeKind::Calls)], 6);
    }

    #[test]
    fn multi_kind_between_same_pair_stays_separate() {
        let agg: HashMap<ShortId, ShortId> = [(10, 2), (20, 5)].into();
        let kinds = kind_map(&[(2, Kind::Class), (5, Kind::Class)]);
        let edges = vec![(10, 20)];
        let mut calls = aggregate_edges_by_kind(&agg, &kinds, &edges, EdgeKind::Calls, 3);
        let type_use = aggregate_edges_by_kind(&agg, &kinds, &edges, EdgeKind::TypeUse, 2);
        calls.extend(type_use);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[&(2, 5, EdgeKind::Calls)], 3);
        assert_eq!(calls[&(2, 5, EdgeKind::TypeUse)], 2);
    }

    #[test]
    fn markdown_links_aggregate_note_to_note() {
        // Section 10 (in note 2) links to section 20 (in note 5). After
        // roll-up both endpoints are their documents → one note→note edge.
        let agg: HashMap<ShortId, ShortId> = [(10, 2), (20, 5)].into();
        let kinds = kind_map(&[(2, Kind::Document), (5, Kind::Document)]);
        let edges = vec![(10, 20)];
        let out = aggregate_edges_by_kind(&agg, &kinds, &edges, EdgeKind::LinksTo, 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[&(2, 5, EdgeKind::LinksTo)], 1);
    }

    #[test]
    fn self_loop_dropped() {
        // Two methods that both roll up to class 2 → self-loop.
        let agg: HashMap<ShortId, ShortId> = [(10, 2), (11, 2)].into();
        let kinds = kind_map(&[(2, Kind::Class)]);
        let edges = vec![(10, 11)];
        let out = aggregate_edges_by_kind(&agg, &kinds, &edges, EdgeKind::Calls, 3);
        assert!(out.is_empty());
    }

    #[test]
    fn opposite_edges_keep_their_direction() {
        let agg: HashMap<ShortId, ShortId> = [(10, 2), (20, 5)].into();
        let kinds = kind_map(&[(2, Kind::Class), (5, Kind::Class)]);
        // Two directed edges going opposite ways → TWO directed rows (they were
        // collapsed to one undirected row before direction was preserved).
        let edges = vec![(10, 20), (20, 10)];
        let out = aggregate_edges_by_kind(&agg, &kinds, &edges, EdgeKind::Calls, 3);
        assert_eq!(out.len(), 2);
        assert_eq!(out[&(2, 5, EdgeKind::Calls)], 3);
        assert_eq!(out[&(5, 2, EdgeKind::Calls)], 3);
    }

    #[test]
    fn imports_dropped_when_either_endpoint_not_module_like() {
        // Class 2 imports class 5 — both class-like, drop.
        let agg: HashMap<ShortId, ShortId> = [(2, 2), (5, 5)].into();
        let kinds = kind_map(&[(2, Kind::Class), (5, Kind::Class)]);
        let edges = vec![(2, 5)];
        let out = aggregate_edges_by_kind(&agg, &kinds, &edges, EdgeKind::Imports, 1);
        assert!(out.is_empty());
    }

    #[test]
    fn imports_kept_when_both_modules() {
        let agg: HashMap<ShortId, ShortId> = [(2, 2), (5, 5)].into();
        let kinds = kind_map(&[(2, Kind::Module), (5, Kind::Module)]);
        let edges = vec![(2, 5)];
        let out = aggregate_edges_by_kind(&agg, &kinds, &edges, EdgeKind::Imports, 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[&(2, 5, EdgeKind::Imports)], 1);
    }

    // ── resolve_anchors ────────────────────────────────────────────

    #[test]
    fn package_anchor_wins_over_path_fallback() {
        let mut s = sym(2, Kind::Class, 0);
        s.pkg_id = 9;
        let syms = map_of(vec![s]);
        let mut files = HashMap::new();
        files.insert(1, "src/lib.rs".to_string());
        let mut pkgs = HashMap::new();
        pkgs.insert(9, "my-pkg".to_string());
        let mut pdf = HashMap::new();
        pdf.insert(2, 1);
        let aggs: HashSet<ShortId> = [2].into();
        let anchors = resolve_anchors(&aggs, &syms, &files, &pkgs, &pdf, &PackageLayout::empty());
        assert_eq!(anchors[&2].1, "my-pkg");
    }

    #[test]
    fn path_fallback_returns_first_segment() {
        let s = sym(2, Kind::Class, 0); // pkg = 0
        let syms = map_of(vec![s]);
        let mut files = HashMap::new();
        files.insert(7, "crates/kenn-indexer/src/transform.rs".to_string());
        let mut pdf = HashMap::new();
        pdf.insert(2, 7);
        let aggs: HashSet<ShortId> = [2].into();
        let anchors = resolve_anchors(
            &aggs,
            &syms,
            &files,
            &HashMap::new(),
            &pdf,
            &PackageLayout::empty(),
        );
        assert_eq!(anchors[&2].1, "crates");
    }

    #[test]
    fn package_layout_marker_wins_over_first_segment() {
        // Path is `crates/kenn-indexer/src/foo.rs`; without a layout
        // the anchor would be the bare first segment `crates`. With a
        // marker at `crates/kenn-indexer` the anchor jumps to that
        // package's declared name.
        use std::path::PathBuf;
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("crates/kenn-indexer")).unwrap();
        std::fs::write(
            root.join("crates/kenn-indexer/Cargo.toml"),
            "[package]\nname = \"kenn-indexer\"\nversion = \"0\"\n",
        )
        .unwrap();
        let layout = PackageLayout::discover(root, &[] as &[PathBuf]);

        let s = sym(2, Kind::Class, 0);
        let syms = map_of(vec![s]);
        let mut files = HashMap::new();
        files.insert(7, "crates/kenn-indexer/src/foo.rs".to_string());
        let mut pdf = HashMap::new();
        pdf.insert(2, 7);
        let aggs: HashSet<ShortId> = [2].into();
        let anchors = resolve_anchors(&aggs, &syms, &files, &HashMap::new(), &pdf, &layout);
        assert_eq!(anchors[&2].1, "kenn-indexer");
    }

    #[test]
    fn missing_both_yields_unanchored() {
        let s = sym(2, Kind::Class, 0);
        let syms = map_of(vec![s]);
        let aggs: HashSet<ShortId> = [2].into();
        let anchors = resolve_anchors(
            &aggs,
            &syms,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &PackageLayout::empty(),
        );
        assert_eq!(anchors[&2].1, "<unanchored>");
    }

    #[test]
    fn anchor_ids_are_deterministic_across_runs() {
        // Build the same input twice; intern ids must match.
        let mut s_a = sym(2, Kind::Class, 0);
        s_a.pkg_id = 5;
        let mut s_b = sym(3, Kind::Class, 0);
        s_b.pkg_id = 7;
        let syms = map_of(vec![s_a, s_b]);
        let mut pkgs = HashMap::new();
        pkgs.insert(5, "alpha".to_string());
        pkgs.insert(7, "beta".to_string());
        let aggs: HashSet<ShortId> = [2, 3].into();
        let a1 = resolve_anchors(
            &aggs,
            &syms,
            &HashMap::new(),
            &pkgs,
            &HashMap::new(),
            &PackageLayout::empty(),
        );
        let a2 = resolve_anchors(
            &aggs,
            &syms,
            &HashMap::new(),
            &pkgs,
            &HashMap::new(),
            &PackageLayout::empty(),
        );
        assert_eq!(a1[&2].0, a2[&2].0);
        assert_eq!(a1[&3].0, a2[&3].0);
        // Sorted name order → "alpha" before "beta" → id 1 vs id 2.
        assert!(a1[&2].0 < a1[&3].0);
    }
}
