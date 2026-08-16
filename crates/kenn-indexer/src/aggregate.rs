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
/// `extends_type` (class/struct inheritance) is kept alongside `implements`:
/// both are "is-a" bonds, and inheritance is real coupling a base class exerts
/// on its subclasses. It is the ONLY form C# inheritance takes (C# emits no
/// `implements` for a base class, ~1.7k `extends_type` edges on a large
/// solution), so dropping it made every C# class hierarchy invisible to the
/// coupling tables and to community detection.
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
    (EdgeKind::ExtendsType, 2),
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

/// Aggregate per-symbol edges of one kind into weighted DIRECTED
/// aggregate-pair edges. Skips self-loops on the aggregate graph and
/// applies the module-to-module-only rule for `imports`.
///
/// Endpoints are NOT sorted: `src → tgt` is kept as-is (see the comment at the
/// insert), because consumers that need direction read it — the atlas `Depends
/// on` links, and the contracts axis, which can only tell an implementer from
/// its interface by the edge's direction.
///
/// Returns a map `(src_agg, tgt_agg, kind) -> total_weight`. Callers
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
                if is_usable_anchor_name(p) {
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

/// Whether a producer-supplied package name can serve as an anchor name.
///
/// A name with no alphanumeric character is a PLACEHOLDER, not a name.
/// scip-python reports the project's own package as `"."` — the project root as
/// a path — and an emptiness check let it through, so the whole repo anchored on
/// `.`: a package concept titled `.` in a file called `python_..md`, with
/// components `python_._src.md` and `python_._docs.md`. Names like those tell a
/// reader nothing and make the id unreadable.
///
/// Rejecting it falls through to the manifest chain, which is strictly better
/// than a salvage: `pyproject.toml` gives the real distribution name, and
/// because markers sort deepest-first, a repo whose examples carry their own
/// manifests splits into those packages instead of collapsing into one anchor.
fn is_usable_anchor_name(name: &str) -> bool {
    name.chars().any(char::is_alphanumeric)
}

/// Full path segments that mark bundled example/sample/demo code. Nodes under
/// one are excluded from domain + central eligibility (like tests), so a demo
/// app referencing a library type never fabricates a "domain" or a "central"
/// symbol.
const EXAMPLE_SEGMENTS: &[&str] = &[
    "example", "examples", "sample", "samples", "demo", "demos", "fixtures",
];

/// Whether `path` lies under an example/sample/demo/fixtures directory segment
/// (case-insensitive, full segment).
///
/// Evaluated here rather than in the atlas because the answer is persisted on
/// the aggregate node: a query over a published snapshot sees no paths, so a
/// consumer-side derivation is not available to every consumer. The atlas keeps
/// one path-level caller — sub-area grouping ranges over all of a package's
/// symbols, not just its aggregate nodes.
#[must_use]
pub(crate) fn is_example_path(path: &str) -> bool {
    path.split(['/', '\\'])
        .any(|seg| EXAMPLE_SEGMENTS.contains(&seg.to_ascii_lowercase().as_str()))
}

/// The EARNED domain count: flat-Louvain communities that clear the domain
/// axis's floors, which is what `kenn domains` lists and what the atlas renders.
///
/// This exists because the overview's `cross_anchor_communities` is the RAW
/// count — every community touching more than one anchor — and the two disagreed
/// by 4x with nothing on either surface saying which was which. Raw communities
/// systematically overstate: they include packages joined only through a shared
/// vendored type, plus one-symbol stragglers.
///
/// Goes through [`crate::atlas::domains::select_domains`], the same rule the
/// atlas producer and the domains query use, so a third surface cannot invent a
/// fourth answer. Reads nothing it isn't given — the caller supplies the
/// communities it read back off the writer's own connection, so this stays
/// `kenn-analyze`-free (the atlas ⊥ analyze rule in `atlas-bundle`).
#[must_use]
pub(crate) fn earned_domain_count(
    nodes: &[AggregateNodeRecord],
    edges: &[AggregateEdgeRecord],
    flat: &[kenn_model::AnalysisFlatCommunityRecord],
    membership: &[kenn_model::AnalysisNodeMembershipRecord],
) -> usize {
    use crate::atlas::domains;

    // First-party + anchored only: an external node is a vendored dependency and
    // `<unanchored>` is the no-package sentinel. Same projection the query makes.
    let node_anchor: HashMap<ShortId, &str> = nodes
        .iter()
        .filter(|n| !n.external && n.anchor_name != UNANCHORED)
        .map(|n| (n.id, n.anchor_name.as_str()))
        .collect();
    // Every eligibility fact comes off the node record — `example` included,
    // since it became a persisted node fact. No file joins needed here.
    let eligible: HashSet<ShortId> = nodes
        .iter()
        .filter(|n| {
            domains::is_domain_eligible(
                &domains::NodeFacts {
                    id: n.id,
                    language: n.language.db_name(),
                    kind: n.kind.db_name(),
                    name: n.name.as_str(),
                    external: n.external,
                    test: n.test,
                    example: n.example,
                },
                node_anchor.contains_key(&n.id),
            )
        })
        .map(|n| n.id)
        .collect();
    let symbol_name: HashMap<ShortId, &str> =
        nodes.iter().map(|n| (n.id, n.name.as_str())).collect();

    // Single-dominant (a monolithic library) keeps within-anchor communities too,
    // matching both other surfaces: strict majority over the eligible set.
    let mut per_anchor: HashMap<&str, usize> = HashMap::new();
    for n in nodes.iter().filter(|n| eligible.contains(&n.id)) {
        if let Some(&a) = node_anchor.get(&n.id) {
            *per_anchor.entry(a).or_default() += 1;
        }
    }
    let total_prod: usize = per_anchor.values().sum();
    let top_prod: usize = per_anchor.values().copied().max().unwrap_or(0);
    let single_dominant = total_prod > 0 && top_prod * 2 > total_prod;

    let keep: HashSet<u32> = flat
        .iter()
        .filter(|f| {
            (f.cross_anchor || single_dominant) && f.size as usize >= domains::MIN_DOMAIN_SIZE
        })
        .map(|f| f.community_id)
        .collect();
    let membership_pairs: Vec<(ShortId, u32)> = membership
        .iter()
        .map(|m| (m.short_id, m.flat_community_id))
        .collect();
    let projected: Vec<domains::Edge> = edges
        .iter()
        .map(|e| domains::Edge {
            src: e.src_id,
            dst: e.dst_id,
            weight: e.weight,
        })
        .collect();

    domains::select_domains(
        &keep,
        &membership_pairs,
        &eligible,
        &projected,
        &node_anchor,
        &symbol_name,
        single_dominant,
    )
    .len()
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
    primary_def_file: &HashMap<ShortId, ShortId>,
    files: &HashMap<ShortId, String>,
) -> Vec<AggregateNodeRecord> {
    let mut out: Vec<AggregateNodeRecord> = participating
        .iter()
        .filter_map(|sid| {
            let s = symbols.get(sid)?;
            let (anchor_id, anchor_name) = anchors
                .get(sid)
                .cloned()
                .unwrap_or((0, UNANCHORED.to_string()));
            // A node with no resolvable def path is not example code — absence
            // of evidence, not evidence of exclusion.
            let example = primary_def_file
                .get(sid)
                .and_then(|f| files.get(f))
                .is_some_and(|p| is_example_path(p));
            Some(AggregateNodeRecord {
                id: s.id,
                kind: s.kind,
                name: s.name.clone(),
                language: s.language,
                external: s.external,
                test: s.test,
                example,
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

/// Raw per-site table edges for the atlas tables axis.
///
/// Raw, not aggregate: which FILE made the reference is that axis's whole
/// answer, and an aggregate edge has already collapsed it.
///
/// A free function rather than three more branches inside
/// `compute_and_persist`, which sits a fraction under the complexity gate and
/// grows every time an axis needs one more input.
async fn scan_table_edges(
    writer: &kenn_store::DbWriter,
) -> Result<Vec<(ShortId, ShortId, EdgeKind)>, kenn_store::api::DbError> {
    let mut out = Vec::new();
    for kind in [
        EdgeKind::DefinesTable,
        EdgeKind::AltersTable,
        EdgeKind::AccessesTable,
    ] {
        for (src, dst) in writer.scan_edges_for_aggregation(kind).await? {
            out.push((src, dst, kind));
        }
    }
    Ok(out)
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
    let nodes = build_aggregate_nodes(
        &participating,
        &symbols,
        &anchors,
        &primary_def_file,
        &files,
    );
    let edges = flatten_edges(per_kind_maps);
    let counts = (nodes.len(), edges.len());
    writer.write_aggregate_tables(&nodes, &edges).await?;

    // The analysis hook runs FIRST: it computes + persists the flat-Louvain
    // communities (`analysis_node_membership` / `analysis_flat_communities`) that
    // the atlas reads back for its `domains` axis. It takes the records by value,
    // so both paths clone — the earned-domain counter below needs them too, and
    // an `AggregateEdgeRecord` is `Copy` while the node clone is a few MB on the
    // largest repo measured. Cheaper than threading projections through the hook.
    aggregated_hook(nodes.clone(), edges.clone(), writer.clone()).await?;

    // The EARNED domain count, as a build-time stat row beside the raw
    // `cross_anchor_communities` the analysis pass writes. Read back on the
    // writer's own connection — the same move the atlas makes, so this adds no
    // dependency on `kenn-analyze` (see `atlas-bundle`: the two stay parallel
    // consumers of the persisted graph).
    //
    // Deliberately OUTSIDE the atlas branch: a counter that appears only on runs
    // that built the atlas is a worse contract than the inconsistency it fixes.
    // Written only when clustering actually produced communities, so "absent"
    // means "analysis did not run" — exactly when the raw counter is absent too,
    // and the two can never be read as disagreeing because one is missing.
    let flat_communities = writer.scan_analysis_flat_communities().await?;
    let node_membership = writer.scan_analysis_node_membership().await?;
    if !flat_communities.is_empty() {
        let earned = earned_domain_count(&nodes, &edges, &flat_communities, &node_membership);
        writer
            .write_stats(&[kenn_store::StatRow {
                scope: "global".to_owned(),
                key: String::new(),
                subset: "graph".to_owned(),
                metric: "domains".to_owned(),
                value: i64::try_from(earned).unwrap_or(i64::MAX),
            }])
            .await?;
    }

    if let Some(ctx) = atlas {
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
        // The domains axis reuses the communities already read back above (atlas ⊥
        // kenn-analyze — it consumes the persisted tables, never recomputes
        // clustering).
        // File-level module docs, to seed each package concept's `description`
        // (atlas tasks 3.4/8.4) verbatim from its root module.
        let file_docs: HashMap<ShortId, String> =
            writer.scan_file_docs().await?.into_iter().collect();
        let table_edges = scan_table_edges(writer).await?;
        let (mut concepts, domains, contracts, tables, shape) =
            crate::atlas::producer::build_concepts(
                &symbols,
                &files,
                &primary_def_file,
                &aggregate_of,
                &atlas_anchors,
                &nodes,
                &edges,
                &node_membership,
                &flat_communities,
                &primary_def_range,
                &file_docs,
                &table_edges,
                &crate::atlas::producer::ShapeMeta {
                    workspace_name: &ctx.workspace_name,
                    freshness: &ctx.freshness,
                    timestamp: &ctx.timestamp,
                },
            );
        // The atlas respects .gitignore: drop any concept whose dir is ignored.
        concepts.retain(|c| !dir_is_gitignored(&ctx.source_root, &c.resource));
        // Refresh the stable `.kenn/atlas` handle here, in the SHARED writer,
        // so `kenn index` and the MCP reindex path both get it (parity).
        // Best-effort: a filesystem that refuses symlinks must not fail a run.
        if let Some(dir) = &ctx.pointer_dir {
            if let Err(e) = crate::atlas::producer::refresh_atlas_pointer(dir, &ctx.out_dir) {
                // Windows without Developer Mode is the expected case. Debug,
                // not warn: the `atlas: <path>` line still names the bundle.
                tracing::debug!(
                    target: "kenn_indexer::atlas",
                    error = %e,
                    "could not refresh the .kenn/atlas pointer"
                );
            }
        }
        crate::atlas::producer::write_bundle(
            &ctx.out_dir,
            &shape,
            &concepts,
            &domains,
            &contracts,
            &tables,
        )
        .map_err(|e| kenn_store::api::DbError::Backend(format!("atlas write: {e}")))?;
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

    // ── earned_domain_count ────────────────────────────────────────

    /// The whole point of the counter: RAW cross-anchor communities overstate.
    /// Here two communities are both flagged `cross_anchor` — the raw count is 2 —
    /// but only one earns the axis. The other spans a second package through a
    /// SINGLE straggler symbol, which is a reference into a cluster, not
    /// membership of it, so `MIN_PKG_MEMBERS` withholds the span and it collapses
    /// to one package.
    ///
    /// This is the 38-vs-9 divergence in miniature, and the reason both numbers
    /// have to be published under their own names.
    ///
    /// Mutation-checked: relaxing `MIN_PKG_MEMBERS` to 1 makes the earned count 2,
    /// i.e. equal to the raw count — only `domains` moves, which is exactly the
    /// property the task asks for.
    #[test]
    fn earned_count_is_below_the_raw_cross_anchor_count() {
        let n = |id: ShortId, name: &str, anchor: &str| AggregateNodeRecord {
            id,
            kind: Kind::Class,
            name: name.to_string(),
            language: Language::Rust,
            external: false,
            test: false,
            example: false,
            anchor_id: 0,
            anchor_name: anchor.to_string(),
        };
        let e = |src: ShortId, dst: ShortId| AggregateEdgeRecord {
            src_id: src,
            dst_id: dst,
            kind: EdgeKind::Calls,
            weight: 3,
        };
        let nodes = vec![
            // community 1 — genuinely spans core + web (2 members each)
            n(1, "A", "core"),
            n(2, "B", "core"),
            n(3, "C", "web"),
            n(4, "D", "web"),
            // community 2 — four in `lib`, ONE straggler in `util`
            n(5, "E", "lib"),
            n(6, "F", "lib"),
            n(7, "G", "lib"),
            n(8, "H", "lib"),
            n(9, "Straggler", "util"),
        ];
        let edges = vec![e(1, 3), e(2, 4), e(5, 9), e(6, 9)];
        let flat = vec![
            kenn_model::AnalysisFlatCommunityRecord {
                community_id: 1,
                size: 4,
                total_weight: 8,
                cross_anchor: true,
                primary_anchor_id: 0,
                primary_anchor_name: "core".into(),
            },
            kenn_model::AnalysisFlatCommunityRecord {
                community_id: 2,
                size: 5,
                total_weight: 10,
                cross_anchor: true,
                primary_anchor_id: 0,
                primary_anchor_name: "lib".into(),
            },
        ];
        let membership: Vec<kenn_model::AnalysisNodeMembershipRecord> = [
            (1u32, 1u32),
            (2, 1),
            (3, 1),
            (4, 1),
            (5, 2),
            (6, 2),
            (7, 2),
            (8, 2),
            (9, 2),
        ]
        .into_iter()
        .map(|(short_id, c)| kenn_model::AnalysisNodeMembershipRecord {
            short_id,
            flat_community_id: c,
            anchored_leaf_community_id: 0,
        })
        .collect();

        let raw = flat.iter().filter(|f| f.cross_anchor).count();
        let earned = earned_domain_count(&nodes, &edges, &flat, &membership);
        assert_eq!(raw, 2, "both communities touch more than one anchor");
        assert_eq!(
            earned, 1,
            "only one clears the floors — a single straggler is not a span"
        );
    }

    /// No communities means the analysis pass did not run, and the caller must
    /// not write a counter at all — an absent row is honest, a `0` reads as "this
    /// repo has no domains".
    #[test]
    fn no_communities_yields_no_domains() {
        assert_eq!(earned_domain_count(&[], &[], &[], &[]), 0);
    }

    // ── build_aggregate_nodes ──────────────────────────────────────

    /// Example-ness is decided HERE, once, and persisted — because the only
    /// other place that could decide it (a query over the published snapshot)
    /// cannot see definition paths. A node under `examples/` is flagged, one
    /// under `src/` is not, and a node with no resolvable def file is not.
    ///
    /// Mutation-checked: hard-coding `example = false` in
    /// `build_aggregate_nodes` fails on the `examples/` node.
    #[test]
    fn example_path_provenance_is_persisted_on_the_node() {
        let syms = map_of(vec![
            sym(1, Kind::Class, 0),
            sym(2, Kind::Class, 0),
            sym(3, Kind::Class, 0),
        ]);
        let mut files = HashMap::new();
        files.insert(10, "crates/store/src/lib.rs".to_string());
        files.insert(11, "crates/store/examples/spike.rs".to_string());
        let mut pdf = HashMap::new();
        pdf.insert(1, 10);
        pdf.insert(2, 11);
        // node 3 deliberately has no def-file entry
        let aggs: HashSet<ShortId> = [1, 2, 3].into();
        let nodes = build_aggregate_nodes(&aggs, &syms, &HashMap::new(), &pdf, &files);

        let flag = |id: ShortId| nodes.iter().find(|n| n.id == id).unwrap().example;
        assert!(!flag(1), "a node under src/ is production code");
        assert!(flag(2), "a node under examples/ is example code");
        assert!(
            !flag(3),
            "no def path is absence of evidence, not evidence of exclusion"
        );
    }

    /// The segment match is whole-segment and case-insensitive, and covers the
    /// Windows separator — a path convention that only held on `/` would flag
    /// nothing on a Windows-indexed workspace.
    #[test]
    fn example_segments_match_whole_segments_either_separator() {
        assert!(is_example_path("crates/store/examples/spike.rs"));
        assert!(is_example_path("crates\\store\\Examples\\spike.rs"));
        assert!(is_example_path("pkg/Fixtures/data.swift"));
        assert!(!is_example_path("crates/store/src/exampled.rs"));
        assert!(!is_example_path("crates/counterexamples/src/lib.rs"));
    }

    // ── resolve_anchors ────────────────────────────────────────────

    /// A package name of `.` is a path placeholder, not a name — scip-python
    /// reports the project's own package that way. It must NOT win the anchor,
    /// or the whole repo anchors on `.` and the atlas writes a concept titled
    /// `.` into `python_..md`. The manifest chain answers instead.
    ///
    /// Mutation-checked: restoring the old `!p.is_empty()` guard makes the
    /// anchor `.` again and fails the first assertion.
    #[test]
    fn a_placeholder_package_name_does_not_win_the_anchor() {
        for placeholder in [".", "..", "", "   ", "./"] {
            let mut s = sym(2, Kind::Class, 0);
            s.pkg_id = 9;
            let syms = map_of(vec![s]);
            let mut files = HashMap::new();
            files.insert(1, "src/app.py".to_string());
            let mut pkgs = HashMap::new();
            pkgs.insert(9, placeholder.to_string());
            let mut pdf = HashMap::new();
            pdf.insert(2, 1);
            let aggs: HashSet<ShortId> = [2].into();
            let anchors =
                resolve_anchors(&aggs, &syms, &files, &pkgs, &pdf, &PackageLayout::empty());
            // No layout marker here, so it falls to the first path segment.
            assert_eq!(
                anchors[&2].1, "src",
                "{placeholder:?} must not become the anchor name"
            );
        }
        // A real name still wins — the guard rejects placeholders, not names.
        for real in ["Flask", "kenn-store", "@acme/web", "Acme.Billing", "v2"] {
            let mut s = sym(2, Kind::Class, 0);
            s.pkg_id = 9;
            let syms = map_of(vec![s]);
            let mut pkgs = HashMap::new();
            pkgs.insert(9, real.to_string());
            let aggs: HashSet<ShortId> = [2].into();
            let anchors = resolve_anchors(
                &aggs,
                &syms,
                &HashMap::new(),
                &pkgs,
                &HashMap::new(),
                &PackageLayout::empty(),
            );
            assert_eq!(anchors[&2].1, real, "a real package name must still win");
        }
    }

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

#[cfg(test)]
mod xml_rollup_tests {
    use super::compute_aggregate_ids;
    use kenn_model::{Kind, Language, ShortId, SymbolRecord};
    use std::collections::HashMap;

    fn el(id: ShortId, kind: Kind, enclosing: ShortId) -> SymbolRecord {
        SymbolRecord {
            id,
            pub_id: format!("xml:doc#e{id}"),
            language: Language::Xml,
            pkg_id: 0,
            kind,
            name: format!("e{id}"),
            enclosing_sym_id: enclosing,
            partial: false,
            nargs: 0,
            targs: 0,
            external: false,
            test: false,
        }
    }

    #[test]
    fn nested_elements_all_roll_up_to_their_document() {
        // The collapse that keeps a numerically dominant document language from
        // distorting the atlas: measured on a real repository, 30410 elements
        // across 485 files become 483 aggregates, 63:1.
        //
        // It works because `XmlElement` is not an aggregate leaf, so an element
        // walks its enclosing chain until it reaches one — and the chain is
        // rooted at the `Document`. If an element ever became a leaf, or the
        // chain stopped terminating there, every element would become its own
        // aggregate and the atlas would be all XML.
        let doc: ShortId = 1;
        let symbols: HashMap<ShortId, SymbolRecord> = [
            el(doc, Kind::Document, 0),
            el(2, Kind::XmlElement, doc), // root
            el(3, Kind::XmlElement, 2),   // child
            el(4, Kind::XmlElement, 3),   // grandchild
        ]
        .into_iter()
        .map(|s| (s.id, s))
        .collect();

        let agg = compute_aggregate_ids(&symbols);
        for id in [2, 3, 4] {
            assert_eq!(
                agg.get(&id),
                Some(&doc),
                "element {id} must aggregate to its document, not to itself"
            );
        }
        assert_eq!(agg.get(&doc), Some(&doc), "the document is its own leaf");
    }
}
