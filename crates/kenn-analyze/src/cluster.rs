//! Single-level Louvain on the aggregated graph, plus an anchored
//! hierarchical view built by recursively re-running Louvain on
//! per-anchor induced subgraphs.
//!
//! Determinism contract: every iteration order is sorted. Same input
//! graph → identical [`Partition`] and identical [`Hierarchy`] structure
//! across runs, including the same level ids.

use std::collections::{BTreeMap, HashMap, HashSet};

use kenn_model::ShortId;

use crate::projection::{AggregatedGraph, AnchorMap};

const MAX_PASSES: usize = 20;

/// Flat Louvain output — a list of communities, each a sorted
/// `Vec<ShortId>`, the list itself sorted by size desc then min member
/// id asc.
pub type Partition = Vec<Vec<ShortId>>;

/// Anchored hierarchy of communities. The root holds one
/// [`HierarchyNode::Anchor`] per anchor (L0). Each anchor branches into
/// Louvain communities computed on its induced subgraph (L1+). Each
/// community below `min_cluster` is a leaf; larger ones recurse up to
/// `max_depth`.
#[derive(Debug, Clone)]
pub struct Hierarchy {
    pub anchors: Vec<AnchorBranch>,
}

#[derive(Debug, Clone)]
pub struct AnchorBranch {
    pub anchor_name: String,
    pub members: Vec<ShortId>,
    pub levels: Vec<HierarchyNode>,
}

#[derive(Debug, Clone)]
pub enum HierarchyNode {
    /// Leaf community (below `min_cluster` or at `max_depth`).
    Leaf { members: Vec<ShortId> },
    /// Internal node carrying its own members + sub-communities.
    Internal {
        members: Vec<ShortId>,
        children: Vec<HierarchyNode>,
    },
}

impl HierarchyNode {
    #[must_use]
    pub fn members(&self) -> &[ShortId] {
        match self {
            Self::Leaf { members } | Self::Internal { members, .. } => members,
        }
    }
}

/// Caller-tunable knobs for [`hierarchical`].
#[derive(Debug, Clone, Copy)]
pub struct HierarchyOptions {
    /// Maximum hierarchy depth, counting the anchor partition as
    /// depth 0. Default 4 — three Louvain levels under each anchor.
    pub max_depth: usize,
    /// Minimum community size to recurse into. Communities below this
    /// threshold are kept as leaf nodes.
    pub min_cluster: usize,
}

impl Default for HierarchyOptions {
    fn default() -> Self {
        Self {
            max_depth: 4,
            min_cluster: 20,
        }
    }
}

/// Run single-level Louvain over the entire aggregated graph
/// (ignoring anchors). Returns the flat partition rendered alongside
/// the anchored hierarchy as a cross-check.
#[must_use]
pub fn louvain_flat(graph: &AggregatedGraph) -> Partition {
    let nodes: HashSet<ShortId> = graph.nodes.keys().copied().collect();
    louvain_induced(graph, &nodes)
}

/// Anchor-partitioned + recursively-clustered hierarchy of the same
/// graph. L0 = anchor partition (no clustering). L1+ = single-level
/// Louvain on each induced subgraph, recursing until `max_depth` or
/// `min_cluster` halts. See [`HierarchyOptions`].
#[must_use]
pub fn hierarchical(
    graph: &AggregatedGraph,
    anchors: &AnchorMap,
    opts: HierarchyOptions,
) -> Hierarchy {
    let mut out = Hierarchy {
        anchors: Vec::with_capacity(anchors.len()),
    };
    for (anchor_name, members) in anchors.iter() {
        let member_set: HashSet<ShortId> = members.iter().copied().collect();
        // First sub-level: Louvain on the induced subgraph.
        let partition = louvain_induced(graph, &member_set);
        let mut levels: Vec<HierarchyNode> = Vec::with_capacity(partition.len());
        for community in partition {
            levels.push(build_subtree(graph, community, 1, opts));
        }
        // Sort branches: size desc, then min member id asc.
        levels.sort_by(|a, b| {
            b.members()
                .len()
                .cmp(&a.members().len())
                .then(a.members().first().cmp(&b.members().first()))
        });
        out.anchors.push(AnchorBranch {
            anchor_name: anchor_name.clone(),
            members: members.clone(),
            levels,
        });
    }
    out
}

fn build_subtree(
    graph: &AggregatedGraph,
    members: Vec<ShortId>,
    depth: usize,
    opts: HierarchyOptions,
) -> HierarchyNode {
    if depth >= opts.max_depth || members.len() < opts.min_cluster {
        return HierarchyNode::Leaf { members };
    }
    let member_set: HashSet<ShortId> = members.iter().copied().collect();
    let partition = louvain_induced(graph, &member_set);
    // If clustering collapsed to one community, no progress — treat as leaf.
    if partition.len() <= 1 {
        return HierarchyNode::Leaf { members };
    }
    let mut children: Vec<HierarchyNode> = Vec::with_capacity(partition.len());
    for sub in partition {
        children.push(build_subtree(graph, sub, depth + 1, opts));
    }
    children.sort_by(|a, b| {
        b.members()
            .len()
            .cmp(&a.members().len())
            .then(a.members().first().cmp(&b.members().first()))
    });
    HierarchyNode::Internal { members, children }
}

/// Members in a STABLE processing order for greedy Louvain — sorted by each
/// node's `(anchor_name, name)` identity, with the `short_id` only as a
/// last-resort tie-break.
///
/// Louvain is order-sensitive and `short_id` numbering is NOT stable across index
/// runs (the same symbols get different ids from interning order), which
/// reshuffled this order and flipped borderline communities on/off between
/// reindexes. Ordering by the node's stable identity makes the partition
/// invariant to id relabeling — see `flat_partition_is_invariant_to_id_relabeling`.
fn stable_order(graph: &AggregatedGraph, members: &HashSet<ShortId>) -> Vec<ShortId> {
    let mut sorted: Vec<ShortId> = members.iter().copied().collect();
    let key = |sid: &ShortId| {
        graph
            .nodes
            .get(sid)
            .map(|n| (n.anchor_name.as_str(), n.name.as_str()))
    };
    sorted.sort_by(|a, b| key(a).cmp(&key(b)).then_with(|| a.cmp(b)));
    sorted
}

/// Single-level Louvain restricted to the given node set. Edges
/// crossing the set boundary are ignored. Used both for the flat
/// pass (over all nodes) and recursively inside [`hierarchical`].
fn louvain_induced(graph: &AggregatedGraph, members: &HashSet<ShortId>) -> Partition {
    if members.is_empty() {
        return Vec::new();
    }
    let sorted_members = stable_order(graph, members);

    // Compute induced degree and 2m.
    let mut induced_degree: HashMap<ShortId, f64> = HashMap::new();
    let mut two_m: f64 = 0.0;
    for &i in &sorted_members {
        let mut d: f64 = 0.0;
        if let Some(nbrs) = graph.adj.get(&i) {
            for &(j, w) in nbrs {
                if !members.contains(&j) {
                    continue;
                }
                d += f64::from(w);
                two_m += f64::from(w);
            }
        }
        induced_degree.insert(i, d);
    }
    if two_m == 0.0 {
        return sorted_members.into_iter().map(|n| vec![n]).collect();
    }
    // Each edge counted twice above (once per endpoint). two_m is
    // already the convention Louvain expects (= 2 * sum_of_edge_weights).

    let mut community: HashMap<ShortId, u32> = sorted_members
        .iter()
        .enumerate()
        .map(|(i, &sid)| (sid, u32::try_from(i).unwrap_or(u32::MAX)))
        .collect();
    let mut tot: HashMap<u32, f64> = HashMap::new();
    for (&sid, &cid) in &community {
        *tot.entry(cid).or_insert(0.0) += induced_degree.get(&sid).copied().unwrap_or(0.0);
    }

    for _pass in 0..MAX_PASSES {
        let mut moved = false;
        for &i in &sorted_members {
            let ki = induced_degree.get(&i).copied().unwrap_or(0.0);
            let ci = community.get(&i).copied().unwrap_or(0);
            let mut k_i_into: BTreeMap<u32, f64> = BTreeMap::new();
            if let Some(nbrs) = graph.adj.get(&i) {
                for &(j, w) in nbrs {
                    if j == i || !members.contains(&j) {
                        continue;
                    }
                    let cj = community.get(&j).copied().unwrap_or(0);
                    *k_i_into.entry(cj).or_insert(0.0) += f64::from(w);
                }
            }
            let k_i_self = k_i_into.get(&ci).copied().unwrap_or(0.0);
            let tot_ci_after_remove = tot.get(&ci).copied().unwrap_or(0.0) - ki;

            let mut best_c = ci;
            let mut best_gain = 0.0_f64;
            for (&c, &k_in) in &k_i_into {
                let tot_c = if c == ci {
                    tot_ci_after_remove
                } else {
                    tot.get(&c).copied().unwrap_or(0.0)
                };
                let gain = k_in - tot_c * ki / two_m;
                if gain > best_gain + f64::EPSILON
                    || (((gain - best_gain).abs() < f64::EPSILON) && c < best_c)
                {
                    best_gain = gain;
                    best_c = c;
                }
            }
            let stay_gain = k_i_self - tot_ci_after_remove * ki / two_m;
            if best_gain > stay_gain + f64::EPSILON && best_c != ci {
                if let Some(t) = tot.get_mut(&ci) {
                    *t -= ki;
                }
                *tot.entry(best_c).or_insert(0.0) += ki;
                community.insert(i, best_c);
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }

    let mut groups: BTreeMap<u32, Vec<ShortId>> = BTreeMap::new();
    for (&sid, &cid) in &community {
        groups.entry(cid).or_default().push(sid);
    }
    let mut as_vec: Vec<Vec<ShortId>> = groups
        .into_values()
        .map(|mut v| {
            v.sort_unstable();
            v
        })
        .collect();
    as_vec.sort_by(|a, b| {
        b.len()
            .cmp(&a.len())
            .then_with(|| a.first().copied().cmp(&b.first().copied()))
    });
    as_vec
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::{AggregatedGraph, NodeInfo};

    fn add_node(g: &mut AggregatedGraph, sid: ShortId, anchor: &str) {
        g.nodes.insert(
            sid,
            NodeInfo {
                kind: "class".into(),
                name: format!("n{sid}"),
                language: "rs".into(),
                external: false,
                test: false,
                anchor_id: 1,
                anchor_name: anchor.into(),
            },
        );
        g.adj.entry(sid).or_default();
    }

    fn add_edge(g: &mut AggregatedGraph, a: ShortId, b: ShortId, w: u32) {
        g.adj.entry(a).or_default().push((b, w));
        g.adj.entry(b).or_default().push((a, w));
        g.edges.push(crate::projection::AggregateEdge {
            a,
            b,
            kind: kenn_model::EdgeKind::Calls,
            weight: w,
        });
        g.total_weight += u64::from(w);
    }

    fn add_named_node(g: &mut AggregatedGraph, sid: ShortId, name: &str, anchor: &str) {
        g.nodes.insert(
            sid,
            NodeInfo {
                kind: "class".into(),
                name: name.into(),
                language: "rs".into(),
                external: false,
                test: false,
                anchor_id: 1,
                anchor_name: anchor.into(),
            },
        );
        g.adj.entry(sid).or_default();
    }

    /// Two triangles bridged by an ambiguous node `G` (weight 2 to each side) —
    /// its community is a near-tie, so the partition is sensitive to processing
    /// order. `perm` maps the structural id to the actual `short_id`, letting a
    /// test re-run the SAME structure/names under a different id numbering.
    fn bridge_graph(perm: impl Fn(ShortId) -> ShortId) -> AggregatedGraph {
        let mut g = AggregatedGraph::default();
        for (sid, name) in [
            (1, "A"),
            (2, "B"),
            (3, "C"),
            (4, "D"),
            (5, "E"),
            (6, "F"),
            (7, "G"),
        ] {
            add_named_node(&mut g, perm(sid), name, "pkg");
        }
        let mut e = |a, b, w| add_edge(&mut g, perm(a), perm(b), w);
        e(1, 2, 5);
        e(2, 3, 5);
        e(1, 3, 5); // triangle A,B,C
        e(4, 5, 5);
        e(5, 6, 5);
        e(4, 6, 5); // triangle D,E,F
        e(3, 7, 2);
        e(4, 7, 2); // G bridges C and D
        g
    }

    fn name_groups(g: &AggregatedGraph, part: &Partition) -> Vec<Vec<String>> {
        let mut out: Vec<Vec<String>> = part
            .iter()
            .map(|c| {
                let mut ns: Vec<String> = c.iter().map(|s| g.nodes[s].name.clone()).collect();
                ns.sort();
                ns
            })
            .collect();
        out.sort();
        out
    }

    /// Louvain is greedy and order-sensitive, and `short_id` numbering is NOT
    /// stable across index runs — so the partition must not depend on it. The same
    /// structure and names under a reversed id numbering must give the same
    /// name-groupings. Mutation-checked: sorting members by `short_id` instead of
    /// the stable `(anchor, name)` key makes these two disagree.
    #[test]
    fn flat_partition_is_invariant_to_id_relabeling() {
        let g1 = bridge_graph(|s| s); // name-order == id-order
        let g2 = bridge_graph(|s| 8 - s); // name-order == reverse of id-order
        assert_eq!(
            name_groups(&g1, &louvain_flat(&g1)),
            name_groups(&g2, &louvain_flat(&g2)),
            "partition must be invariant to short_id relabeling"
        );
    }

    #[test]
    fn flat_empty_is_empty() {
        let g = AggregatedGraph::default();
        assert!(louvain_flat(&g).is_empty());
    }

    #[test]
    fn flat_two_triangles_split_into_two() {
        let mut g = AggregatedGraph::default();
        for sid in 1..=6 {
            add_node(&mut g, sid, "a");
        }
        add_edge(&mut g, 1, 2, 5);
        add_edge(&mut g, 2, 3, 5);
        add_edge(&mut g, 1, 3, 5);
        add_edge(&mut g, 4, 5, 5);
        add_edge(&mut g, 5, 6, 5);
        add_edge(&mut g, 4, 6, 5);
        add_edge(&mut g, 3, 4, 1);
        let groups = louvain_flat(&g);
        assert_eq!(groups.len(), 2);
        let sizes: Vec<usize> = groups.iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![3, 3]);
    }

    #[test]
    fn hierarchical_two_anchors_keep_separate_l0() {
        let mut g = AggregatedGraph::default();
        for sid in 1..=4 {
            add_node(&mut g, sid, "alpha");
        }
        for sid in 5..=8 {
            add_node(&mut g, sid, "beta");
        }
        add_edge(&mut g, 1, 2, 3);
        add_edge(&mut g, 3, 4, 3);
        add_edge(&mut g, 5, 6, 3);
        add_edge(&mut g, 7, 8, 3);
        let am = AnchorMap::from_graph(&g);
        let h = hierarchical(&g, &am, HierarchyOptions::default());
        assert_eq!(h.anchors.len(), 2);
        let names: Vec<_> = h.anchors.iter().map(|a| a.anchor_name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn hierarchical_min_cluster_halts_recursion() {
        let mut g = AggregatedGraph::default();
        for sid in 1..=10 {
            add_node(&mut g, sid, "a");
        }
        for i in 1..10 {
            add_edge(&mut g, i, i + 1, 1);
        }
        let am = AnchorMap::from_graph(&g);
        let h = hierarchical(
            &g,
            &am,
            HierarchyOptions {
                max_depth: 4,
                min_cluster: 100,
            },
        );
        // min_cluster=100 means every community under the anchor stays leaf.
        let only = &h.anchors[0];
        for l in &only.levels {
            assert!(matches!(l, HierarchyNode::Leaf { .. }));
        }
    }

    #[test]
    fn hierarchical_determinism_across_calls() {
        // Compare shape by serializing branches' member ids.
        fn shape(h: &Hierarchy) -> Vec<(String, Vec<Vec<ShortId>>)> {
            h.anchors
                .iter()
                .map(|br| {
                    let levels: Vec<Vec<ShortId>> =
                        br.levels.iter().map(|n| n.members().to_vec()).collect();
                    (br.anchor_name.clone(), levels)
                })
                .collect()
        }
        let mut g = AggregatedGraph::default();
        for sid in 1..=8 {
            add_node(&mut g, sid, "a");
        }
        add_edge(&mut g, 1, 2, 3);
        add_edge(&mut g, 2, 3, 3);
        add_edge(&mut g, 3, 4, 1);
        add_edge(&mut g, 4, 5, 3);
        add_edge(&mut g, 5, 6, 3);
        add_edge(&mut g, 6, 7, 1);
        add_edge(&mut g, 7, 8, 3);
        let am = AnchorMap::from_graph(&g);
        let opts = HierarchyOptions::default();
        let a = hierarchical(&g, &am, opts);
        let b = hierarchical(&g, &am, opts);
        assert_eq!(shape(&a), shape(&b));
    }
}
