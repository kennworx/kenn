//! Anchor coupling construction + initial seeding: weighted degrees,
//! coupling graph, disc packing, and the spectral / sunflower seeds.

use std::collections::{BTreeMap, HashMap};

use kenn_model::ShortId;

use crate::projection::AggregatedGraph;

use super::{
    enforce_non_overlap, force_layout, linlog_layout, relax_with_couplings, stress_layout,
    LayoutAlgo, ANCHOR_GAP, GOLDEN_ANGLE,
};

pub(crate) fn compute_weighted_degrees(graph: &AggregatedGraph) -> BTreeMap<ShortId, u64> {
    graph
        .nodes
        .keys()
        .map(|sid| (*sid, graph.weighted_degree(*sid)))
        .collect()
}

/// Build symmetric anchor-anchor coupling (summed cross-anchor edge
/// weight). Returned as adjacency list sorted by neighbor index — the
/// stable order is what makes the relaxation byte-deterministic across
/// runs.
#[expect(
    clippy::cast_precision_loss,
    reason = "edge weights are bounded well below 2^23"
)]
#[expect(
    clippy::indexing_slicing,
    reason = "ai/bi come from node_anchor which only maps into [0, n_anchors)"
)]
pub(crate) fn build_anchor_couplings(
    graph: &AggregatedGraph,
    node_anchor: &HashMap<ShortId, usize>,
    n_anchors: usize,
) -> Vec<Vec<(usize, f32)>> {
    let mut acc: Vec<BTreeMap<usize, f32>> = (0..n_anchors).map(|_| BTreeMap::new()).collect();
    for e in &graph.edges {
        let (Some(&ai), Some(&bi)) = (node_anchor.get(&e.a), node_anchor.get(&e.b)) else {
            continue;
        };
        if ai == bi {
            continue;
        }
        let w = e.weight as f32;
        *acc[ai].entry(bi).or_insert(0.0) += w;
        *acc[bi].entry(ai).or_insert(0.0) += w;
    }
    acc.into_iter().map(|m| m.into_iter().collect()).collect()
}

/// Coupling-aware anchor placement.
///
/// Strategy: a **spectral seed** computes 2D coordinates from the
/// top two non-trivial eigenvectors of the random-walk smoothing
/// matrix (deflated power iteration). Strongly-coupled clusters end
/// up close in this embedding by construction — much better global
/// structure than what local force iteration can find on its own.
///
/// We then run a short force-refinement pass to spread out from the
/// degenerate-direction collapse spectral methods sometimes produce,
/// and finally a non-overlap cleanup. No RNG.
pub(crate) fn pack_anchor_centers(
    radii: &[f32],
    couplings: &[Vec<(usize, f32)>],
    algo: LayoutAlgo,
) -> Vec<(f32, f32)> {
    let n = radii.len();
    if n == 0 {
        return Vec::new();
    }
    let coupled = n >= 3 && has_any_coupling(couplings);
    // Every coupled algorithm starts from the spectral embedding —
    // strongly-coupled anchors land near each other in the initial
    // position so the local refinement pass can't trap a pair on
    // opposite sides of the canvas via the repulsion field of every
    // other cluster they don't care about.
    let mut centers = if coupled {
        spectral_seed(radii, couplings)
    } else {
        sunflower_seed(radii)
    };
    if n > 1 {
        match algo {
            LayoutAlgo::Spectral => relax_with_couplings(&mut centers, radii, couplings),
            LayoutAlgo::Force => force_layout(&mut centers, radii, couplings),
            LayoutAlgo::Stress => stress_layout(&mut centers, radii, couplings),
            LayoutAlgo::LinLog => linlog_layout(&mut centers, radii, couplings),
        }
    }
    enforce_non_overlap(&mut centers, radii);
    centers
}

fn has_any_coupling(couplings: &[Vec<(usize, f32)>]) -> bool {
    couplings.iter().any(|v| !v.is_empty())
}

/// 2D spectral embedding of the anchor super-graph.
///
/// We power-iterate the random-walk smoothing matrix `M = D⁻¹ W`,
/// projecting out the trivial constant component each step so the
/// iteration converges to the second eigenvector of `M` (algebraic
/// connectivity direction). Then we repeat with the result also
/// projected out to get the third eigenvector. The `(eig₂, eig₃)`
/// pair becomes the (x, y) coordinates: strongly-coupled anchors land
/// at nearby coordinates because their walk-mixing values are close.
///
/// Cost: O(ITERS × (N + `E_anchor`)) per dimension. For N=366 anchors
/// and `E_anchor` ≈ several thousand this is sub-millisecond.
#[expect(
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    reason = "indices come from parallel-length vectors; anchor count well below 2^23"
)]
fn spectral_seed(radii: &[f32], couplings: &[Vec<(usize, f32)>]) -> Vec<(f32, f32)> {
    const ITERS: usize = 120;
    let n = radii.len();

    // Random-walk degree (sum of coupling weights). Floored at 1 so
    // isolated anchors don't divide by zero.
    let deg: Vec<f32> = (0..n)
        .map(|i| couplings[i].iter().map(|(_, w)| *w).sum::<f32>().max(1.0))
        .collect();

    // Deterministic non-constant seeds. Two different irrational
    // multipliers give linearly-independent initial directions, which
    // power iteration then "rotates" into the dominant eigenvectors.
    let mut x: Vec<f32> = (0..n).map(|i| ((i as f32) * 1.732_050_8).cos()).collect();
    let mut y: Vec<f32> = (0..n).map(|i| ((i as f32) * 2.236_068).sin()).collect();

    for _ in 0..ITERS {
        x = smooth(&x, couplings, &deg);
        center_and_normalize(&mut x);
        y = smooth(&y, couplings, &deg);
        // Orthogonalize y against both the constant direction (mean=0)
        // and against x — otherwise both vectors converge to the same
        // dominant eigenvector.
        center_and_normalize(&mut y);
        let dot: f32 = x.iter().zip(y.iter()).map(|(a, b)| a * b).sum();
        for i in 0..n {
            y[i] -= dot * x[i];
        }
        center_and_normalize(&mut y);
    }

    // Scale to a canvas roughly proportional to total disc area, so
    // discs aren't crammed into a tiny ball.
    let total_area: f32 = radii
        .iter()
        .map(|r| std::f32::consts::PI * (r + ANCHOR_GAP).powi(2))
        .sum();
    let canvas_r = (total_area * 1.6 / std::f32::consts::PI).sqrt();
    let ext = x
        .iter()
        .chain(y.iter())
        .map(|v| v.abs())
        .fold(0.0_f32, f32::max)
        .max(1e-6);
    let scale = canvas_r / ext;

    (0..n).map(|i| (x[i] * scale, y[i] * scale)).collect()
}

/// One step of random-walk smoothing: `out[i] = Σⱼ wᵢⱼ/dᵢ · v[j]`.
#[expect(
    clippy::indexing_slicing,
    reason = "all indices come from parallel-length vectors"
)]
fn smooth(v: &[f32], couplings: &[Vec<(usize, f32)>], deg: &[f32]) -> Vec<f32> {
    let n = v.len();
    let mut out = vec![0.0_f32; n];
    for i in 0..n {
        let mut acc = 0.0_f32;
        for &(j, w) in &couplings[i] {
            acc += w * v[j];
        }
        out[i] = acc / deg[i];
    }
    out
}

/// Subtract mean (project out constants) and rescale to unit L2 norm.
#[expect(clippy::cast_precision_loss, reason = "n is well below 2^23")]
fn center_and_normalize(v: &mut [f32]) {
    let n = v.len();
    if n == 0 {
        return;
    }
    let mean = v.iter().sum::<f32>() / n as f32;
    for x in v.iter_mut() {
        *x -= mean;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    for x in v.iter_mut() {
        *x /= norm;
    }
}

/// Initial sunflower-spiral seed: anchors land at evenly-spread
/// directions on a spiral whose radius grows with cumulative area.
#[expect(clippy::cast_precision_loss, reason = "anchor index well below 2^23")]
fn sunflower_seed(radii: &[f32]) -> Vec<(f32, f32)> {
    let mut out = Vec::with_capacity(radii.len());
    let mut cumulative_area: f32 = 0.0;
    for (i, &r) in radii.iter().enumerate() {
        cumulative_area += std::f32::consts::PI * (r + ANCHOR_GAP).powi(2);
        let base_r = (cumulative_area / std::f32::consts::PI).sqrt() * 0.85;
        let theta = (i as f32) * GOLDEN_ANGLE;
        out.push((base_r * theta.cos(), base_r * theta.sin()));
    }
    out
}
