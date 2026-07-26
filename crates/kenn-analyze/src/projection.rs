//! The aggregated graph that all downstream analysis runs against.
//!
//! Two paths produce one shape:
//!
//! - `load_from_reader` reads `scan_aggregate_*` from a snapshot that
//!   has the Phase 2 artifact. O(rows on disk) — cheap.
//! - `build` (the prototype path) scans symbols + per-kind edges and
//!   recomputes the projection in memory. Fallback for snapshots that
//!   pre-date the aggregate tables.
//!
//! The struct carries both per-kind edges (for rendering / kind-aware
//! analysis) and a collapsed adjacency (for Louvain weight maths).

use std::collections::{BTreeMap, HashMap};

use kenn_model::{EdgeKind, ShortId};
use kenn_store::api::types::{AggregateEdgeRow, AggregateNodeRow, DbError};
use kenn_store::api::Reader;

#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub kind: String,
    pub name: String,
    pub language: String,
    /// `true` when the symbol comes from a package marked external —
    /// used to split god-node reporting into "user" vs "system" sets.
    pub external: bool,
    /// `true` when the symbol was indexed from a path matching a test
    /// glob (or a Rust descriptor heuristic). Set at ingest time.
    pub test: bool,
    /// Interned anchor id (stable per snapshot). 0 when no anchor.
    pub anchor_id: u32,
    /// Human-readable anchor label (`kenn-indexer`, `<unanchored>`, …).
    pub anchor_name: String,
}

/// One aggregate-graph edge of a specific kind. The aggregated graph is
/// per-kind by construction: `(a, b, calls)` and `(a, b, type_use)` are
/// distinct edges, each with its own weight.
#[derive(Debug, Clone)]
pub struct AggregateEdge {
    pub a: ShortId,
    pub b: ShortId,
    pub kind: EdgeKind,
    pub weight: u32,
}

/// The aggregated weighted undirected graph that downstream analysis
/// (Louvain, hierarchical clustering, god-nodes, render) consumes.
#[derive(Debug, Default)]
pub struct AggregatedGraph {
    pub nodes: HashMap<ShortId, NodeInfo>,
    /// Per-(pair, kind) edges in the order they were loaded.
    pub edges: Vec<AggregateEdge>,
    /// Collapsed adjacency: for each node, list of `(neighbor, weight)`
    /// where weight is the sum across all kinds for that pair. Used
    /// directly by Louvain — clustering doesn't care about kinds.
    pub adj: HashMap<ShortId, Vec<(ShortId, u32)>>,
    /// Total edge weight (each undirected edge counted once, summed
    /// across all kinds).
    pub total_weight: u64,
}

impl AggregatedGraph {
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Distinct undirected edges, counting each `(pair, kind)` once.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Weighted degree summed across all incident kinds.
    #[must_use]
    pub fn weighted_degree(&self, node: ShortId) -> u64 {
        self.adj
            .get(&node)
            .map_or(0, |nbrs| nbrs.iter().map(|&(_, w)| u64::from(w)).sum())
    }

    /// True when the loaded artifact contained no rows (snapshot
    /// pre-dates Phase 2 → fall back to `build`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.edges.is_empty()
    }

    /// A first-party-only copy of the graph for FLAT community detection.
    ///
    /// The atlas maps project code, so its domain axis must cluster project code
    /// — not the shared stdlib/vendored types first-party symbols happen to
    /// mention. External nodes (and every edge incident to one) are dropped, so
    /// two first-party symbols land in one community because THEY reference each
    /// other, never because they both use `HashMap`. Downstream already ignores
    /// external nodes (the atlas filters them on read; `build_membership_records`
    /// assigns anything absent from a community the unassigned sentinel), so this
    /// only aligns the clustering INPUT with what the output already consumes —
    /// today we cluster with the vendor glue, then throw the glue away and keep
    /// the first-party members it grouped.
    ///
    /// The flat pass alone uses this; god-node degree and the anchored hierarchy
    /// keep the full graph — external god-nodes are reported on purpose, and the
    /// hierarchy is per-anchor, so external symbols already sit in their own
    /// branches.
    ///
    /// Edge weights are re-scaled by kind ([`cluster_kind_weight`]): the
    /// aggregate weight encodes a coupling-traffic prior (a call outweighs an
    /// implements), but community detection wants the opposite — an
    /// `implements`/`overrides` bond means "same contract" and should pull two
    /// symbols together harder than an incidental call.
    #[must_use]
    pub fn clustering_view(&self) -> Self {
        let nodes: HashMap<ShortId, NodeInfo> = self
            .nodes
            .iter()
            .filter(|(_, n)| !n.external)
            .map(|(&id, n)| (id, n.clone()))
            .collect();
        let mut edges: Vec<AggregateEdge> = Vec::new();
        let mut per_pair: HashMap<(ShortId, ShortId), u32> = HashMap::new();
        let mut total_weight: u64 = 0;
        for e in &self.edges {
            if !nodes.contains_key(&e.a) || !nodes.contains_key(&e.b) {
                continue;
            }
            let w = e.weight.saturating_mul(cluster_kind_weight(e.kind));
            edges.push(AggregateEdge {
                a: e.a,
                b: e.b,
                kind: e.kind,
                weight: w,
            });
            let (lo, hi) = (e.a.min(e.b), e.a.max(e.b));
            *per_pair.entry((lo, hi)).or_insert(0) += w;
            total_weight += u64::from(w);
        }
        let mut adj: HashMap<ShortId, Vec<(ShortId, u32)>> =
            nodes.keys().map(|&k| (k, Vec::new())).collect();
        for ((a, b), w) in per_pair {
            adj.entry(a).or_default().push((b, w));
            adj.entry(b).or_default().push((a, w));
        }
        Self {
            nodes,
            edges,
            adj,
            total_weight,
        }
    }
}

/// Per-kind multiplier applied to the aggregate edge weight when building the
/// FLAT-clustering adjacency ([`AggregatedGraph::clustering_view`]).
///
/// The aggregate weight encodes a coupling-TRAFFIC prior — a `call` (base 3)
/// outweighs an `implements` (base 2) — which is right for the coupling tables
/// but backwards for community detection. An is-a bond (`implements` /
/// `overrides` / `extends_type`) means "same contract / same hierarchy" and is a
/// far stronger signal that two symbols belong to one concept than an incidental
/// call. Boosting the is-a family ×4 flips the effective order (a lone
/// `implements` at 2·4=8 outpulls two calls at 3), so an interface clusters with
/// its implementers — the relationship the domain axis exists to surface.
///
/// No language has all three: Go/Python carry only `implements`, Swift adds
/// `overrides`, C# inheritance is `extends_type`. Weighting the family covers
/// whichever each language emits.
#[must_use]
const fn cluster_kind_weight(kind: EdgeKind) -> u32 {
    match kind {
        EdgeKind::Implements | EdgeKind::Overrides | EdgeKind::ExtendsType => 4,
        _ => 1,
    }
}

/// Anchor → set of aggregate ids inside it. Used as L0 of the
/// anchored hierarchy. Iteration order is anchor name ascending
/// (deterministic — `BTreeMap`).
#[derive(Debug, Clone, Default)]
pub struct AnchorMap {
    by_anchor: BTreeMap<String, Vec<ShortId>>,
}

impl AnchorMap {
    /// Build from an `AggregatedGraph` — group each node by its
    /// `anchor_name`. Nodes with the same anchor land in one bucket,
    /// sorted by `short_id` for deterministic clustering.
    #[must_use]
    pub fn from_graph(graph: &AggregatedGraph) -> Self {
        let mut by_anchor: BTreeMap<String, Vec<ShortId>> = BTreeMap::new();
        for (sid, info) in &graph.nodes {
            by_anchor
                .entry(info.anchor_name.clone())
                .or_default()
                .push(*sid);
        }
        for ids in by_anchor.values_mut() {
            ids.sort_unstable();
        }
        Self { by_anchor }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Vec<ShortId>)> {
        self.by_anchor.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_anchor.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_anchor.is_empty()
    }
}

/// Which slice of nodes to return from [`top_by_weighted_degree`].
#[derive(Debug, Clone, Copy)]
pub enum NodeFilter {
    All,
    /// User code that is NOT a test (`external = false && test = false`).
    UserLiveOnly,
    /// User test code (`external = false && test = true`).
    UserTestOnly,
    /// Only nodes whose backing symbol is external.
    ExternalOnly,
}

/// Compute a top-N list ordered by weighted degree, ties broken by `short_id`.
#[must_use]
pub fn top_by_weighted_degree(
    graph: &AggregatedGraph,
    n: usize,
    filter: NodeFilter,
) -> Vec<(ShortId, u64)> {
    let mut all: Vec<(ShortId, u64)> = graph
        .nodes
        .iter()
        .filter(|(_, info)| match filter {
            NodeFilter::All => true,
            NodeFilter::UserLiveOnly => !info.external && !info.test,
            NodeFilter::UserTestOnly => !info.external && info.test,
            NodeFilter::ExternalOnly => info.external,
        })
        .map(|(&sid, _)| (sid, graph.weighted_degree(sid)))
        .collect();
    all.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    all.truncate(n);
    all
}

/// Load the aggregated graph from a snapshot's `aggregate_nodes` /
/// `aggregate_edges` tables. Returns an empty graph (not an error)
/// when the snapshot pre-dates Phase 2 — callers detect via
/// [`AggregatedGraph::is_empty`] and fall back to [`build`].
pub async fn load_from_reader<R: Reader>(reader: &R) -> Result<AggregatedGraph, DbError> {
    let node_rows: Vec<AggregateNodeRow> = reader.scan_aggregate_nodes().await?;
    let edge_rows: Vec<AggregateEdgeRow> = reader.scan_aggregate_edges().await?;
    let mut graph = AggregatedGraph {
        nodes: HashMap::with_capacity(node_rows.len()),
        edges: Vec::with_capacity(edge_rows.len()),
        adj: HashMap::with_capacity(node_rows.len()),
        total_weight: 0,
    };
    for n in node_rows {
        graph.nodes.insert(
            n.id,
            NodeInfo {
                kind: n.kind,
                name: n.name,
                language: n.language,
                external: n.external,
                test: n.test,
                anchor_id: n.anchor_id,
                anchor_name: n.anchor_name,
            },
        );
        graph.adj.entry(n.id).or_default();
    }
    let mut per_pair: HashMap<(ShortId, ShortId), u32> = HashMap::new();
    for e in edge_rows {
        graph.edges.push(AggregateEdge {
            a: e.src_id,
            b: e.dst_id,
            kind: e.kind,
            weight: e.weight,
        });
        // Collapse to undirected for Louvain / weighted-degree: opposite
        // directions between the same pair sum into one adjacency weight.
        let (lo, hi) = (e.src_id.min(e.dst_id), e.src_id.max(e.dst_id));
        *per_pair.entry((lo, hi)).or_insert(0) += e.weight;
        graph.total_weight += u64::from(e.weight);
    }
    for ((a, b), w) in per_pair {
        graph.adj.entry(a).or_default().push((b, w));
        graph.adj.entry(b).or_default().push((a, w));
    }
    Ok(graph)
}

/// Build an `AggregatedGraph` directly from in-memory aggregate
/// records. Used by the indexer's post-aggregation hook so the
/// just-computed `(nodes, edges)` don't need to round-trip through
/// the store. The shape matches `load_from_reader` byte-for-byte.
#[must_use]
pub fn build_from_records(
    nodes: &[kenn_model::AggregateNodeRecord],
    edges: &[kenn_model::AggregateEdgeRecord],
) -> AggregatedGraph {
    let mut graph = AggregatedGraph {
        nodes: HashMap::with_capacity(nodes.len()),
        edges: Vec::with_capacity(edges.len()),
        adj: HashMap::with_capacity(nodes.len()),
        total_weight: 0,
    };
    for n in nodes {
        graph.nodes.insert(
            n.id,
            NodeInfo {
                kind: n.kind.db_name().to_string(),
                name: n.name.clone(),
                language: n.language.db_name().to_string(),
                external: n.external,
                test: n.test,
                anchor_id: n.anchor_id,
                anchor_name: n.anchor_name.clone(),
            },
        );
        graph.adj.entry(n.id).or_default();
    }
    let mut per_pair: HashMap<(ShortId, ShortId), u32> = HashMap::new();
    for e in edges {
        graph.edges.push(AggregateEdge {
            a: e.src_id,
            b: e.dst_id,
            kind: e.kind,
            weight: e.weight,
        });
        // Collapse to undirected for Louvain / weighted-degree.
        let (lo, hi) = (e.src_id.min(e.dst_id), e.src_id.max(e.dst_id));
        *per_pair.entry((lo, hi)).or_insert(0) += e.weight;
        graph.total_weight += u64::from(e.weight);
    }
    for ((a, b), w) in per_pair {
        graph.adj.entry(a).or_default().push((b, w));
        graph.adj.entry(b).or_default().push((a, w));
    }
    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenn_model::EdgeKind;

    fn node(external: bool) -> NodeInfo {
        NodeInfo {
            kind: "class".into(),
            name: "N".into(),
            language: "rs".into(),
            external,
            test: false,
            anchor_id: 0,
            anchor_name: "a".into(),
        }
    }

    /// The clustering view drops external nodes and every edge incident to one,
    /// and re-weights the is-a family up while leaving calls alone. Weight of the
    /// surviving 1↔2 pair = implements(2)·4 + calls(3)·1 = 11. Mutation-checked:
    /// dropping the `!external` filter keeps node 3; dropping the is-a boost makes
    /// the pair weight 2+3=5.
    #[test]
    fn clustering_view_excludes_external_and_boosts_is_a() {
        let mut g = AggregatedGraph::default();
        g.nodes.insert(1, node(false));
        g.nodes.insert(2, node(false));
        g.nodes.insert(3, node(true)); // external
        g.edges = vec![
            AggregateEdge {
                a: 1,
                b: 2,
                kind: EdgeKind::Implements,
                weight: 2,
            },
            AggregateEdge {
                a: 1,
                b: 2,
                kind: EdgeKind::Calls,
                weight: 3,
            },
            AggregateEdge {
                a: 2,
                b: 3,
                kind: EdgeKind::Calls,
                weight: 3,
            }, // to external
        ];
        let v = g.clustering_view();
        assert!(!v.nodes.contains_key(&3), "external node dropped");
        assert_eq!(v.nodes.len(), 2);
        // 2→3 edge (incident to external) is gone; 1↔2 keeps implements·4 + calls.
        let w: u32 = v.adj[&1]
            .iter()
            .find(|&&(n, _)| n == 2)
            .map(|&(_, w)| w)
            .unwrap();
        assert_eq!(w, 2 * 4 + 3, "is-a boosted ×4, calls unchanged");
        assert_eq!(v.edges.len(), 2, "only the two internal edges survive");
    }
}
