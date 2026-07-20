//! Public layout surface: algorithm selection, the `Layout` result, the
//! shared geometric constants, and the `compute` orchestrator.

use std::collections::{BTreeMap, HashMap};

use kenn_model::ShortId;

use crate::projection::{AggregatedGraph, AnchorMap};

use super::{
    build_anchor_couplings, compute_weighted_degrees, node_offset_in_disc, pack_anchor_centers,
};

/// Layout algorithm selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutAlgo {
    #[default]
    /// Spectral 2D embedding (eigenvectors of the random-walk
    /// smoothing matrix) + short force refinement + non-overlap pass.
    /// Good global structure for moderate-coupling graphs; can place a
    /// weakly-connected pair on opposite sides when their eigenvector
    /// coordinates happen to differ in sign.
    Spectral,
    /// Fruchterman-Reingold-style force layout seeded from the
    /// sunflower spiral. Strong spring attraction along couplings so a
    /// cluster with only one connection ends up directly adjacent to
    /// that neighbor, plus quadratic repulsion to spread the rest.
    Force,
    /// Stress majorization seeded from the spectral embedding. Computes
    /// all-pairs shortest-path graph distances on the anchor
    /// super-graph (Dijkstra with `1/√weight` edge lengths) and
    /// iteratively pulls every pair toward its graph-distance-matched
    /// Euclidean position. Produces clean cluster separation that
    /// respects the actual reachability metric — best choice for
    /// dense, community-rich graphs like large C# enterprise repos.
    Stress,
    /// `Noack`'s `LinLog` model: constant per-edge attraction
    /// (`F_a = w`, not FR's `d²/k`) so weakly-coupled pairs are still
    /// pulled together regardless of distance; logarithmic repulsion
    /// (`F_r ∝ 1/d`) so distant clusters barely push each other.
    /// Different mathematical attractor from spectral / stress — try
    /// this when the same anchor pair you expect to cluster appears
    /// scattered under those algorithms.
    LinLog,
}

impl LayoutAlgo {
    /// Parse `"spectral"` or `"force"`. Case-insensitive.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "spectral" => Some(Self::Spectral),
            "force" => Some(Self::Force),
            "stress" => Some(Self::Stress),
            "linlog" => Some(Self::LinLog),
            _ => None,
        }
    }
}

/// Resulting position table. Browser-side renderer uses these as
/// preset coordinates with no further layout work.
#[derive(Debug, Default)]
pub struct Layout {
    /// Per-node positions: `(short_id, x, y)`. Used for detail-view
    /// rendering.
    pub positions: Vec<(ShortId, f32, f32)>,
    /// Per-anchor disc info: `(anchor_name, center_x, center_y, radius)`.
    /// Used by the overview-mode renderer as supernode positions.
    pub anchor_discs: Vec<(String, f32, f32, f32)>,
}

/// Golden angle in radians (~ 137.5°). Used for sunflower-style
/// placements where consecutive items land at evenly-distributed
/// directions.
pub(crate) const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// Pixels per unit of `sqrt(node_count)`. Controls how big each anchor
/// disc gets relative to its node count.
const NODE_AREA_UNIT: f32 = 22.0;
/// Padding between adjacent anchor discs.
pub(crate) const ANCHOR_GAP: f32 = 60.0;
/// Minimum radius for tiny anchors so a 1-node anchor still has room
/// for its label.
const MIN_ANCHOR_RADIUS: f32 = 35.0;

/// Compute static positions for every node in `graph`. Anchors are
/// circle-packed onto a sunflower spiral; nodes within each anchor
/// land on a Fermat spiral inside the anchor's disc.
#[must_use]
#[expect(
    clippy::indexing_slicing,
    reason = "indices come from the enumerated anchor_entries, always in bounds for the parallel radii / centers vectors"
)]
pub fn compute(graph: &AggregatedGraph, anchors: &AnchorMap, algo: LayoutAlgo) -> Layout {
    let degree_by_node = compute_weighted_degrees(graph);

    // Anchor order: size desc (so the bigger ones get placed first
    // and end up nearer the centroid), tiebreaker anchor name asc.
    let mut anchor_entries: Vec<(&String, &Vec<ShortId>)> = anchors.iter().collect();
    anchor_entries.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));

    // Anchor disc radii — area ∝ node count.
    #[expect(clippy::cast_precision_loss, reason = "node count well below 2^23")]
    let anchor_radii: Vec<f32> = anchor_entries
        .iter()
        .map(|(_, m)| {
            let r = (m.len() as f32).sqrt() * NODE_AREA_UNIT;
            r.max(MIN_ANCHOR_RADIUS)
        })
        .collect();

    // Build anchor-anchor coupling (summed cross-anchor edge weights) so
    // strongly-connected clusters get pulled toward each other during
    // the relaxation pass below.
    let anchor_index_by_name: BTreeMap<&str, usize> = anchor_entries
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (name.as_str(), i))
        .collect();
    let node_anchor: HashMap<ShortId, usize> = graph
        .nodes
        .iter()
        .filter_map(|(sid, info)| {
            anchor_index_by_name
                .get(info.anchor_name.as_str())
                .map(|&i| (*sid, i))
        })
        .collect();
    let couplings = build_anchor_couplings(graph, &node_anchor, anchor_entries.len());

    let anchor_centers = pack_anchor_centers(&anchor_radii, &couplings, algo);

    // Expose disc geometry so the renderer can place supernodes in
    // overview mode.
    let anchor_discs: Vec<(String, f32, f32, f32)> = anchor_entries
        .iter()
        .enumerate()
        .map(|(i, (name, _))| {
            (
                (*name).clone(),
                anchor_centers[i].0,
                anchor_centers[i].1,
                anchor_radii[i],
            )
        })
        .collect();

    // Place nodes inside each anchor's disc on a Fermat spiral. Don't
    // pin the heaviest node at the center (that's what was causing
    // the visual hub).
    let mut positions: Vec<(ShortId, f32, f32)> = Vec::with_capacity(graph.nodes.len());
    for (idx, (_name, members)) in anchor_entries.iter().enumerate() {
        let (cx, cy) = anchor_centers[idx];
        let disc_r = anchor_radii[idx];

        let mut sorted = (*members).clone();
        sorted.sort_by(|a, b| {
            let da = degree_by_node.get(a).copied().unwrap_or(0);
            let db = degree_by_node.get(b).copied().unwrap_or(0);
            db.cmp(&da).then(a.cmp(b))
        });

        let n = sorted.len();
        for (i, sid) in sorted.iter().enumerate() {
            let (dx, dy) = node_offset_in_disc(i, n, disc_r);
            positions.push((*sid, cx + dx, cy + dy));
        }
    }

    Layout {
        positions,
        anchor_discs,
    }
}
