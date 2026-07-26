//! Graph analysis layer over a published kenn snapshot.
//!
//! Reads the aggregated graph (`scan_aggregate_*`) when the snapshot
//! has the Phase 2 artifact; falls back to scanning per-symbol records
//! and recomputing the projection in memory when it doesn't. On top
//! of that runs:
//!
//!   - god-node ranking split by live / test / external,
//!   - flat Louvain over the first-party view (external nodes excluded,
//!     is-a edges up-weighted — see `AggregatedGraph::clustering_view`),
//!   - anchored hierarchical Louvain (L0 = anchor, L1+ = recursive
//!     Louvain on induced subgraphs),
//!
//! and provides `kenn visualize`'s graph (`[index] graph_path`, default
//! `kenn_graph.html`).

pub mod aggregate;
pub mod cluster;
pub mod graph;
pub mod layout;
pub mod projection;

use std::collections::HashMap;

use kenn_model::{
    AggregateEdgeRecord, AggregateNodeRecord, AnalysisAnchoredCommunityRecord,
    AnalysisFlatCommunityRecord, AnalysisGodNodeRecord, AnalysisNodeMembershipRecord,
    GodNodeFilter, Kind, ShortId,
};
use kenn_store::api::DbError;
use kenn_store::{DbWriter, StatRow};

use crate::cluster::{Hierarchy, HierarchyNode, HierarchyOptions, Partition};
use crate::projection::{AggregatedGraph, AnchorMap, NodeFilter};

/// Knobs for the pure analysis pipeline — what `compute_analysis`
/// reads. Matches the CLI defaults the previous `kenn analyze`
/// command exposed; index-time callers will read these from `[index]
/// analysis.*` config.
#[derive(Debug, Clone, Copy)]
pub struct AnalysisOptions {
    pub top_n: usize,
    pub max_depth: usize,
    pub min_cluster: usize,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            top_n: 20,
            max_depth: 4,
            min_cluster: 20,
        }
    }
}

/// Output of [`compute_analysis`]: every derived fact the report and
/// future readers (MCP tools, the visualize command) need, in memory.
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub anchors: AnchorMap,
    pub hierarchy: Hierarchy,
    pub flat: Partition,
    pub god_live: Vec<(ShortId, u64)>,
    pub god_test: Vec<(ShortId, u64)>,
    pub god_external: Vec<(ShortId, u64)>,
}

/// Pure analysis pipeline: anchor map → anchored hierarchical Louvain
/// → flat Louvain → god-node rankings. No IO. Deterministic for a
/// given `(graph, opts)`.
#[must_use]
pub fn compute_analysis(graph: &AggregatedGraph, opts: &AnalysisOptions) -> AnalysisResult {
    let anchors = AnchorMap::from_graph(graph);
    let hierarchy = cluster::hierarchical(
        graph,
        &anchors,
        HierarchyOptions {
            max_depth: opts.max_depth,
            min_cluster: opts.min_cluster,
        },
    );
    // Flat community detection runs on the first-party, kind-reweighted view —
    // the atlas domain axis maps project code, not the vendored/stdlib types it
    // references. God-nodes and the hierarchy below keep the full `graph`.
    let flat = cluster::louvain_flat(&graph.clustering_view());
    let god_live = projection::top_by_weighted_degree(graph, opts.top_n, NodeFilter::UserLiveOnly);
    let god_test = projection::top_by_weighted_degree(graph, opts.top_n, NodeFilter::UserTestOnly);
    let god_external =
        projection::top_by_weighted_degree(graph, opts.top_n, NodeFilter::ExternalOnly);
    AnalysisResult {
        anchors,
        hierarchy,
        flat,
        god_live,
        god_test,
        god_external,
    }
}

/// Counts a caller can pull from an [`AnalysisResult`] for status
/// reporting (CLI stdout, MCP overview, etc.).
#[must_use]
pub fn analysis_counts(graph: &AggregatedGraph, r: &AnalysisResult) -> AnalysisCounts {
    AnalysisCounts {
        nodes: graph.node_count(),
        edges: graph.edge_count(),
        anchors: r.hierarchy.anchors.len(),
        flat_communities: r.flat.len(),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AnalysisCounts {
    pub nodes: usize,
    pub edges: usize,
    pub anchors: usize,
    pub flat_communities: usize,
}

/// Same shape as `kenn_indexer::pipeline::PostAggregateHook` but
/// without the dep on `kenn-indexer`. Boxed `FnOnce` returning a boxed
/// future — the storage writer surface is async (retire-redb D9). It
/// takes the records and a writer clone by value so the future is
/// `'static` + `Send`.
pub type AnalysisHook = Box<
    dyn FnOnce(
            Vec<AggregateNodeRecord>,
            Vec<AggregateEdgeRecord>,
            DbWriter,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), DbError>> + Send>>
        + Send,
>;

/// Build a boxed post-aggregate hook that runs the full analysis
/// pipeline (`build_from_records` → `compute_analysis` → `to_records`
/// → `Writer::write_analysis_tables`) and records the graph-structure
/// counters. Plug into `kenn_indexer::index_workspace` or
/// `run_pipeline_with_progress`.
#[must_use]
pub fn build_analysis_hook(opts: AnalysisOptions) -> AnalysisHook {
    Box::new(move |nodes, edges, writer| {
        Box::pin(async move {
            let graph = projection::build_from_records(&nodes, &edges);
            let result = compute_analysis(&graph, &opts);
            let recs = to_records(&graph, &result);
            writer
                .write_analysis_tables(
                    &recs.god_nodes,
                    &recs.flat,
                    &recs.anchored,
                    &recs.membership,
                )
                .await?;
            // Graph-structure counters fall out of the records just built
            // (build-time-stats): per-language node/hub/community/anchor counts
            // and the whole-graph hierarchy depth + cross-anchor count, under
            // subset='graph'. No extra traversal.
            writer.write_stats(&graph_stat_rows(&nodes, &recs)).await?;
            Ok(())
        })
    })
}

/// Build the post-aggregate hook from config: the full analysis hook
/// (analysis tables + graph stats) when `index.persist_analysis` is set,
/// else a no-op. Centralizes the gate so every orchestration path — CLI
/// `kenn index` and the MCP in-process reindex — persists analysis
/// identically. A path that builds its own hook risks silently dropping
/// the analysis tables and graph stats on reindex (which is exactly what
/// an in-process reindex did before this).
#[must_use]
pub fn analysis_hook_from_config(config: &kenn_config::Config) -> AnalysisHook {
    if config.index.persist_analysis {
        build_analysis_hook(AnalysisOptions {
            top_n: config.index.analysis.top_n,
            max_depth: config.index.analysis.max_depth,
            min_cluster: config.index.analysis.min_cluster,
        })
    } else {
        Box::new(|_nodes, _edges, _writer| Box::pin(async { Ok(()) }))
    }
}

/// Plurality language of a `lang → count` tally — the language with the most
/// members, ties broken by name for determinism. `None` if empty.
fn plurality_lang(counts: &std::collections::HashMap<&'static str, usize>) -> Option<&'static str> {
    counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(l, _)| *l)
}

/// Graph-structure stat rows from the analysis records (build-time-stats).
/// Per-language counts under `subset='graph'` — `nodes`, `god_nodes`,
/// `communities` and `anchors` (each attributed to the plurality language of
/// its member nodes; an anchor/community spans languages and has none itself) —
/// plus the whole-graph `hierarchy_depth` and `cross_anchor_communities` under
/// `scope='global'`. Derived from records already built; no graph traversal.
fn graph_stat_rows(nodes: &[AggregateNodeRecord], recs: &AnalysisRecords) -> Vec<StatRow> {
    use std::collections::{HashMap, HashSet};

    // Node id → language. (Anchor ids are a DIFFERENT id space — never look an
    // anchor id up here; attribute via member nodes instead.)
    let node_lang: HashMap<ShortId, &'static str> =
        nodes.iter().map(|n| (n.id, n.language.db_name())).collect();
    // (language, metric) → count
    let mut per_lang: HashMap<(&'static str, &'static str), i64> = HashMap::new();

    for n in nodes {
        *per_lang.entry((n.language.db_name(), "nodes")).or_default() += 1;
    }
    // god nodes ARE aggregate nodes (short_id is a node id), so attribute directly.
    let mut seen_god = HashSet::new();
    for g in &recs.god_nodes {
        if seen_god.insert(g.short_id) {
            if let Some(&l) = node_lang.get(&g.short_id) {
                *per_lang.entry((l, "god_nodes")).or_default() += 1;
            }
        }
    }
    // communities: plurality language of each flat community's member nodes
    // (membership has one row per node → its flat_community_id).
    let mut comm_langs: HashMap<u32, HashMap<&'static str, usize>> = HashMap::new();
    for m in &recs.membership {
        if let Some(&l) = node_lang.get(&m.short_id) {
            *comm_langs
                .entry(m.flat_community_id)
                .or_default()
                .entry(l)
                .or_default() += 1;
        }
    }
    for langs in comm_langs.values() {
        if let Some(l) = plurality_lang(langs) {
            *per_lang.entry((l, "communities")).or_default() += 1;
        }
    }
    // anchors: plurality language of each anchor's member nodes, grouped by the
    // stable interned `anchor_id` (names can collide); the no-anchor bucket (id
    // 0) is excluded.
    let mut anchor_langs: HashMap<u32, HashMap<&'static str, usize>> = HashMap::new();
    for n in nodes {
        if n.anchor_id == 0 {
            continue; // unanchored
        }
        *anchor_langs
            .entry(n.anchor_id)
            .or_default()
            .entry(n.language.db_name())
            .or_default() += 1;
    }
    for langs in anchor_langs.values() {
        if let Some(l) = plurality_lang(langs) {
            *per_lang.entry((l, "anchors")).or_default() += 1;
        }
    }

    let mut rows: Vec<StatRow> = per_lang
        .into_iter()
        .map(|((lang, metric), value)| StatRow {
            scope: "language".to_owned(),
            key: lang.to_owned(),
            subset: "graph".to_owned(),
            metric: metric.to_owned(),
            value,
        })
        .collect();

    let depth = recs.anchored.iter().map(|a| a.depth).max().unwrap_or(0);
    let cross =
        i64::try_from(recs.flat.iter().filter(|f| f.cross_anchor).count()).unwrap_or(i64::MAX);
    for (metric, value) in [
        ("hierarchy_depth", i64::from(depth)),
        ("cross_anchor_communities", cross),
    ] {
        rows.push(StatRow {
            scope: "global".to_owned(),
            key: String::new(),
            subset: "graph".to_owned(),
            metric: metric.to_owned(),
            value,
        });
    }
    rows
}

/// Tuple of all four record vectors the indexer persists via the
/// writer's `write_analysis_tables` operation. Built by [`to_records`].
pub struct AnalysisRecords {
    pub god_nodes: Vec<AnalysisGodNodeRecord>,
    pub flat: Vec<AnalysisFlatCommunityRecord>,
    pub anchored: Vec<AnalysisAnchoredCommunityRecord>,
    pub membership: Vec<AnalysisNodeMembershipRecord>,
}

/// Flatten an [`AnalysisResult`] into the four record vectors that
/// match the snapshot's analysis tables 1:1. Pure — no IO.
///
/// - `god_nodes`: one row per `(filter, rank)`. Name / kind /
///   anchor are looked up from `graph.nodes`; `kind` rows that don't
///   parse as a `Kind` are skipped (shouldn't happen — every
///   aggregate node has a valid kind).
/// - `flat`: one row per flat-Louvain community. `community_id` is
///   the partition's array index. `cross_anchor` flips true when
///   members span more than one anchor name. `primary_anchor_*` is
///   the plurality anchor among the members.
/// - `anchored`: depth-first flatten of the anchored hierarchy.
///   Depth-0 rows are the anchor roots (`parent_id = None`). Each
///   tree node gets a unique `community_id`.
/// - `membership`: one row per aggregate node. `flat_community_id`
///   comes from the flat partition; `anchored_leaf_community_id` is
///   the deepest community in the anchored tree that still contains
///   the node (we walk the recursion top-down and remember the
///   deepest hit).
#[must_use]
pub fn to_records(graph: &AggregatedGraph, r: &AnalysisResult) -> AnalysisRecords {
    let god_nodes = build_god_records(graph, r);
    let (flat, flat_membership) = build_flat_records(graph, &r.flat);
    let (anchored, anchored_leaf) = build_anchored_records(&r.hierarchy);
    let membership = build_membership_records(graph, &flat_membership, &anchored_leaf);
    AnalysisRecords {
        god_nodes,
        flat,
        anchored,
        membership,
    }
}

fn build_god_records(graph: &AggregatedGraph, r: &AnalysisResult) -> Vec<AnalysisGodNodeRecord> {
    let mut out = Vec::with_capacity(r.god_live.len() + r.god_test.len() + r.god_external.len());
    push_god(graph, &mut out, GodNodeFilter::Live, &r.god_live);
    push_god(graph, &mut out, GodNodeFilter::Test, &r.god_test);
    push_god(graph, &mut out, GodNodeFilter::External, &r.god_external);
    out
}

fn push_god(
    graph: &AggregatedGraph,
    out: &mut Vec<AnalysisGodNodeRecord>,
    filter: GodNodeFilter,
    list: &[(ShortId, u64)],
) {
    for (rank, &(short_id, weighted_degree)) in list.iter().enumerate() {
        let Some(info) = graph.nodes.get(&short_id) else {
            continue;
        };
        let Some(kind) = parse_kind(&info.kind) else {
            continue;
        };
        out.push(AnalysisGodNodeRecord {
            filter,
            // `rank` from enumerate is `usize` but the column is u32 —
            // we trim N below 2^32 (top_n caps in the hundreds at most).
            #[expect(clippy::cast_possible_truncation, reason = "rank bounded by top_n")]
            rank: rank as u32,
            short_id,
            weighted_degree,
            name: info.name.clone(),
            kind,
            anchor_id: info.anchor_id,
            anchor_name: info.anchor_name.clone(),
        });
    }
}

fn parse_kind(s: &str) -> Option<Kind> {
    Kind::from_db_name(s)
}

fn build_flat_records(
    graph: &AggregatedGraph,
    flat: &Partition,
) -> (Vec<AnalysisFlatCommunityRecord>, HashMap<ShortId, u32>) {
    let mut out = Vec::with_capacity(flat.len());
    let mut membership: HashMap<ShortId, u32> = HashMap::with_capacity(graph.nodes.len());
    for (idx, members) in flat.iter().enumerate() {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "community count bounded by node count"
        )]
        let community_id = idx as u32;
        let mut anchor_counts: HashMap<&str, u32> = HashMap::new();
        let mut total_weight: u64 = 0;
        for &sid in members {
            membership.insert(sid, community_id);
            if let Some(info) = graph.nodes.get(&sid) {
                *anchor_counts.entry(info.anchor_name.as_str()).or_insert(0) += 1;
                total_weight += graph.weighted_degree(sid);
            }
        }
        let (primary_anchor_name, _primary_count) = anchor_counts
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| a.0.cmp(b.0)))
            .map_or(("<unanchored>", 0), |(name, count)| (*name, *count));
        let primary_anchor_id = members
            .iter()
            .filter_map(|sid| graph.nodes.get(sid))
            .find(|info| info.anchor_name == primary_anchor_name)
            .map_or(0, |info| info.anchor_id);
        let cross_anchor = anchor_counts.len() > 1;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "size bounded by node count"
        )]
        let size = members.len() as u32;
        out.push(AnalysisFlatCommunityRecord {
            community_id,
            size,
            total_weight,
            cross_anchor,
            primary_anchor_id,
            primary_anchor_name: primary_anchor_name.to_string(),
        });
    }
    (out, membership)
}

fn build_anchored_records(
    hierarchy: &Hierarchy,
) -> (Vec<AnalysisAnchoredCommunityRecord>, HashMap<ShortId, u32>) {
    let mut out = Vec::new();
    let mut leaf_membership: HashMap<ShortId, u32> = HashMap::new();
    let mut next_id: u32 = 0;
    for branch in &hierarchy.anchors {
        let anchor_root_id = next_id;
        next_id += 1;
        let anchor_id = 0; // anchor names are interned at index time; we don't
                           // have the id in the hierarchy. The lookup happens at row-build time
                           // in build_membership_records via graph.nodes, so leave 0 here and
                           // the indexer's caller can backfill via the anchor map if needed.
        let (test_ratio, test_infra) = compute_test_ratio_branch(&branch.members);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "branch size bounded by node count"
        )]
        let size = branch.members.len() as u32;
        out.push(AnalysisAnchoredCommunityRecord {
            community_id: anchor_root_id,
            parent_id: None,
            depth: 0,
            anchor_id,
            anchor_name: branch.anchor_name.clone(),
            size,
            test_ratio,
            test_infra,
        });
        // Default leaf assignment for every member is the anchor root.
        // Deeper recursion below may overwrite when the member lands in
        // a sub-community.
        for &sid in &branch.members {
            leaf_membership.insert(sid, anchor_root_id);
        }
        for node in &branch.levels {
            flatten_anchored(
                node,
                anchor_root_id,
                1,
                &branch.anchor_name,
                &mut out,
                &mut leaf_membership,
                &mut next_id,
            );
        }
    }
    (out, leaf_membership)
}

fn compute_test_ratio_branch(_members: &[ShortId]) -> (f32, bool) {
    // Branch-level test ratio requires per-member `test` flags which
    // the `Hierarchy` doesn't carry. The indexer fills this in when
    // it has the graph in scope; we default to 0 here and let
    // `build_membership_records` post-process if needed. The persisted
    // row stays available for tools that want a quick lookup.
    (0.0, false)
}

fn flatten_anchored(
    node: &HierarchyNode,
    parent_id: u32,
    depth: u32,
    anchor_name: &str,
    out: &mut Vec<AnalysisAnchoredCommunityRecord>,
    leaf: &mut HashMap<ShortId, u32>,
    next_id: &mut u32,
) {
    let community_id = *next_id;
    *next_id += 1;
    let (members, children): (&[ShortId], &[HierarchyNode]) = match node {
        HierarchyNode::Leaf { members } => (members, &[]),
        HierarchyNode::Internal { members, children } => (members, children),
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "size bounded by node count"
    )]
    let size = members.len() as u32;
    out.push(AnalysisAnchoredCommunityRecord {
        community_id,
        parent_id: Some(parent_id),
        depth,
        anchor_id: 0,
        anchor_name: anchor_name.to_string(),
        size,
        test_ratio: 0.0,
        test_infra: false,
    });
    // Overwrite leaf assignment for every member at this depth — the
    // deepest community wins (children recurse last).
    for &sid in members {
        leaf.insert(sid, community_id);
    }
    for child in children {
        flatten_anchored(
            child,
            community_id,
            depth + 1,
            anchor_name,
            out,
            leaf,
            next_id,
        );
    }
}

fn build_membership_records(
    graph: &AggregatedGraph,
    flat: &HashMap<ShortId, u32>,
    anchored_leaf: &HashMap<ShortId, u32>,
) -> Vec<AnalysisNodeMembershipRecord> {
    let mut out = Vec::with_capacity(graph.nodes.len());
    let mut ids: Vec<ShortId> = graph.nodes.keys().copied().collect();
    ids.sort_unstable();
    for sid in ids {
        out.push(AnalysisNodeMembershipRecord {
            short_id: sid,
            flat_community_id: flat.get(&sid).copied().unwrap_or(u32::MAX),
            anchored_leaf_community_id: anchored_leaf.get(&sid).copied().unwrap_or(u32::MAX),
        });
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::{AggregateEdge, AggregatedGraph, NodeInfo};
    use std::collections::HashMap;

    fn fixture() -> AggregatedGraph {
        // Three anchors, two members each. Edges connect inside and
        // across anchors so flat Louvain produces > 1 community.
        let mut nodes: HashMap<ShortId, NodeInfo> = HashMap::new();
        let mk = |kind: &str, name: &str, anchor: &str, external: bool, test: bool| NodeInfo {
            kind: kind.into(),
            name: name.into(),
            language: "rs".into(),
            external,
            test,
            anchor_id: 0,
            anchor_name: anchor.into(),
        };
        nodes.insert(1, mk("class", "A1", "alpha", false, false));
        nodes.insert(2, mk("class", "A2", "alpha", false, false));
        nodes.insert(3, mk("class", "B1", "beta", false, true));
        nodes.insert(4, mk("class", "B2", "beta", false, false));
        nodes.insert(5, mk("class", "X", "ext", true, false));
        nodes.insert(6, mk("class", "Y", "ext", true, false));
        let edges = vec![
            AggregateEdge {
                a: 1,
                b: 2,
                kind: kenn_model::EdgeKind::Calls,
                weight: 3,
            },
            AggregateEdge {
                a: 3,
                b: 4,
                kind: kenn_model::EdgeKind::Calls,
                weight: 3,
            },
            AggregateEdge {
                a: 5,
                b: 6,
                kind: kenn_model::EdgeKind::TypeUse,
                weight: 2,
            },
            AggregateEdge {
                a: 2,
                b: 3,
                kind: kenn_model::EdgeKind::Calls,
                weight: 1,
            },
            AggregateEdge {
                a: 4,
                b: 5,
                kind: kenn_model::EdgeKind::TypeUse,
                weight: 1,
            },
        ];
        let mut adj: HashMap<ShortId, Vec<(ShortId, u32)>> = HashMap::new();
        for sid in nodes.keys() {
            adj.insert(*sid, Vec::new());
        }
        let mut total_weight = 0_u64;
        for e in &edges {
            adj.entry(e.a).or_default().push((e.b, e.weight));
            adj.entry(e.b).or_default().push((e.a, e.weight));
            total_weight += u64::from(e.weight);
        }
        AggregatedGraph {
            nodes,
            edges,
            adj,
            total_weight,
        }
    }

    #[test]
    fn compute_analysis_is_deterministic() {
        let g = fixture();
        let opts = AnalysisOptions::default();
        let r1 = compute_analysis(&g, &opts);
        let r2 = compute_analysis(&g, &opts);
        assert_eq!(r1.god_live, r2.god_live);
        assert_eq!(r1.god_test, r2.god_test);
        assert_eq!(r1.god_external, r2.god_external);
        assert_eq!(r1.flat, r2.flat);
        assert_eq!(r1.hierarchy.anchors.len(), r2.hierarchy.anchors.len());
    }

    /// Flat community detection runs on the first-party view, so no external
    /// node (5, 6 in the fixture) may appear in any flat community — even though
    /// they are edge-connected (5↔6, 4→5). They still get a membership row
    /// downstream (the unassigned sentinel), which `to_records_shapes_match_graph`
    /// covers. Mutation-checked: clustering over the full graph puts 5 and 6 in a
    /// community here.
    #[test]
    fn flat_communities_exclude_external_nodes() {
        let g = fixture();
        let r = compute_analysis(&g, &AnalysisOptions::default());
        let clustered: std::collections::HashSet<ShortId> =
            r.flat.iter().flatten().copied().collect();
        assert!(
            !clustered.contains(&5),
            "external node 5 must not be clustered"
        );
        assert!(
            !clustered.contains(&6),
            "external node 6 must not be clustered"
        );
        assert!(
            clustered.contains(&1),
            "first-party nodes are still clustered"
        );
    }

    #[test]
    fn to_records_shapes_match_graph() {
        let g = fixture();
        let r = compute_analysis(&g, &AnalysisOptions::default());
        let recs = to_records(&g, &r);
        // god_nodes: at most 3 * top_n; with 6 nodes, expect every
        // matching node to appear (live = 2, test = 1, external = 2 ⇒ 5).
        assert!(recs.god_nodes.len() <= 3 * 20);
        // flat: as many rows as communities.
        assert_eq!(recs.flat.len(), r.flat.len());
        // anchored: at least one row per anchor (depth-0 root).
        assert!(recs.anchored.len() >= r.hierarchy.anchors.len());
        // membership: exactly one row per aggregate node.
        assert_eq!(recs.membership.len(), g.nodes.len());
    }

    #[test]
    fn graph_stat_rows_attributes_per_language_and_whole_graph() {
        use kenn_model::Language;
        let node = |id: u32, lang: Language, anchor: u32| AggregateNodeRecord {
            id,
            kind: Kind::Function,
            name: format!("n{id}"),
            language: lang,
            external: false,
            test: false,
            example: false,
            anchor_id: anchor,
            anchor_name: format!("a{anchor}"),
        };
        // rust nodes 1,2 + anchor 10; csharp node 3 + anchor 20.
        let nodes = vec![
            node(1, Language::Rust, 10),
            node(2, Language::Rust, 10),
            node(10, Language::Rust, 10),
            node(3, Language::Csharp, 20),
            node(20, Language::Csharp, 20),
        ];
        let recs = AnalysisRecords {
            god_nodes: vec![AnalysisGodNodeRecord {
                filter: GodNodeFilter::Live,
                rank: 0,
                short_id: 1, // a rust node
                weighted_degree: 0,
                name: "n1".into(),
                kind: Kind::Function,
                anchor_id: 10,
                anchor_name: "a10".into(),
            }],
            flat: vec![
                AnalysisFlatCommunityRecord {
                    community_id: 0,
                    size: 2,
                    total_weight: 0,
                    cross_anchor: false,
                    primary_anchor_id: 10, // rust
                    primary_anchor_name: "a10".into(),
                },
                AnalysisFlatCommunityRecord {
                    community_id: 1,
                    size: 2,
                    total_weight: 0,
                    cross_anchor: true,
                    primary_anchor_id: 20, // csharp
                    primary_anchor_name: "a20".into(),
                },
            ],
            anchored: vec![AnalysisAnchoredCommunityRecord {
                community_id: 0,
                parent_id: None,
                depth: 2,
                anchor_id: 10,
                anchor_name: "a10".into(),
                size: 2,
                test_ratio: 0.0,
                test_infra: false,
            }],
            // community 0 = rust nodes 1,2; community 1 = csharp node 3.
            membership: vec![
                AnalysisNodeMembershipRecord {
                    short_id: 1,
                    flat_community_id: 0,
                    anchored_leaf_community_id: 0,
                },
                AnalysisNodeMembershipRecord {
                    short_id: 2,
                    flat_community_id: 0,
                    anchored_leaf_community_id: 0,
                },
                AnalysisNodeMembershipRecord {
                    short_id: 3,
                    flat_community_id: 1,
                    anchored_leaf_community_id: 0,
                },
            ],
        };
        let rows = super::graph_stat_rows(&nodes, &recs);
        let get = |scope: &str, key: &str, metric: &str| {
            rows.iter()
                .find(|s| {
                    s.scope == scope && s.key == key && s.subset == "graph" && s.metric == metric
                })
                .map(|s| s.value)
        };
        assert_eq!(get("language", "rust", "nodes"), Some(3));
        assert_eq!(get("language", "csharp", "nodes"), Some(2));
        assert_eq!(get("language", "rust", "god_nodes"), Some(1));
        assert_eq!(get("language", "rust", "communities"), Some(1));
        assert_eq!(get("language", "csharp", "communities"), Some(1));
        assert_eq!(get("language", "rust", "anchors"), Some(1));
        assert_eq!(get("global", "", "hierarchy_depth"), Some(2));
        assert_eq!(get("global", "", "cross_anchor_communities"), Some(1));
    }
}
