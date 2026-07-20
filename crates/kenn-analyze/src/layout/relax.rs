//! Force / linlog / stress relaxation passes over the anchor super-graph,
//! all-pairs Dijkstra for stress targets, and the final non-overlap +
//! in-disc node placement.

use super::{ANCHOR_GAP, GOLDEN_ANGLE};

/// Iterative force layout on the anchor super-graph. Coupled anchors
/// attract; anchors closer than touching repel. Cools down linearly.
///
/// Pull strength uses **per-anchor normalization**: each anchor's
/// strongest coupling gets full pull, regardless of how that weight
/// compares to the global max. Without this, a tiny cluster whose
/// only edges go to one neighbor sees `w/max_w_global ≈ 0` and never
/// migrates close to that neighbor.
#[expect(
    clippy::indexing_slicing,
    reason = "indices come from the parallel radii / couplings / forces vectors, all of length n"
)]
pub(crate) fn relax_with_couplings(
    centers: &mut [(f32, f32)],
    radii: &[f32],
    couplings: &[Vec<(usize, f32)>],
) {
    // Short refinement only — the spectral seed already provides the
    // global structure; this pass mainly spreads things out so the
    // overlap cleanup has less work to do.
    const ITERS: usize = 80;
    let n = centers.len();

    // Per-anchor max coupling weight. Used as the denominator so each
    // anchor's strongest tie is normalized to 1.0.
    let per_anchor_max: Vec<f32> = (0..n)
        .map(|i| {
            couplings[i]
                .iter()
                .map(|(_, w)| *w)
                .fold(0.0_f32, f32::max)
                .max(1.0)
        })
        .collect();

    for iter in 0..ITERS {
        #[expect(clippy::cast_precision_loss, reason = "ITERS is a small const")]
        let t = 1.0 - (iter as f32 / ITERS as f32);
        let mut forces = vec![(0.0_f32, 0.0_f32); n];

        // Attractive forces along couplings. The normalized weight uses
        // whichever endpoint sees this as its strongest tie — so a tiny
        // cluster with one dominant neighbor gets full-strength pull.
        for i in 0..n {
            for &(j, w) in &couplings[i] {
                if j <= i {
                    continue;
                }
                let dx = centers[j].0 - centers[i].0;
                let dy = centers[j].1 - centers[i].1;
                let d = dx.hypot(dy).max(1.0);
                let touch = radii[i] + radii[j] + ANCHOR_GAP;
                let norm = (w / per_anchor_max[i]).max(w / per_anchor_max[j]);
                let pull = (d - touch) * norm.sqrt() * 0.18 * t;
                let fx = pull * dx / d;
                let fy = pull * dy / d;
                forces[i].0 += fx;
                forces[i].1 += fy;
                forces[j].0 -= fx;
                forces[j].1 -= fy;
            }
        }

        // Soft repulsion between all pairs — collapses are prevented and
        // touching pairs get a small additional separation. The constant
        // is lower than the pull coefficient above so couplings win.
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = centers[j].0 - centers[i].0;
                let dy = centers[j].1 - centers[i].1;
                let d = dx.hypot(dy).max(0.5);
                let min_d = radii[i] + radii[j] + ANCHOR_GAP;
                if d < min_d * 1.3 {
                    let push = (min_d - d).max(0.0) * 0.4 * t;
                    let fx = push * dx / d;
                    let fy = push * dy / d;
                    forces[i].0 -= fx;
                    forces[i].1 -= fy;
                    forces[j].0 += fx;
                    forces[j].1 += fy;
                }
            }
        }

        for k in 0..n {
            // Cap per-iteration step to keep the system from oscillating
            // when t is high and a few clusters have very stretched
            // springs.
            let max_step = (radii[k] * 0.5).max(40.0);
            let fx = forces[k].0.clamp(-max_step, max_step);
            let fy = forces[k].1.clamp(-max_step, max_step);
            centers[k].0 += fx;
            centers[k].1 += fy;
        }
    }
}

/// Classical Fruchterman-Reingold force layout, weighted by per-anchor
/// normalized coupling. For a cluster with only one coupling, the
/// attraction pulls it directly adjacent to that neighbor (repulsion
/// from unrelated clusters falls off as `1/d`, so it can't compete
/// with the `d²/k` attraction from a strong spring).
#[expect(
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    reason = "indices are bounded by n; n well below 2^23"
)]
pub(crate) fn force_layout(
    centers: &mut [(f32, f32)],
    radii: &[f32],
    couplings: &[Vec<(usize, f32)>],
) {
    const ITERS: usize = 400;
    // Hooke-like pull toward origin; see use site.
    const GRAVITY: f32 = 0.08;
    let n = centers.len();
    if n < 2 {
        return;
    }

    // Optimal pair distance `k`: bigger when discs are bigger or the
    // graph is larger. Combination of average disc + per-anchor area
    // budget keeps the canvas readable at all scales.
    let avg_r: f32 = radii.iter().sum::<f32>() / n as f32;
    let total_area: f32 = radii
        .iter()
        .map(|r| (r + ANCHOR_GAP).powi(2) * std::f32::consts::PI)
        .sum();
    let k = (total_area / n as f32).sqrt().max(avg_r * 2.5);
    // Hard boundary: no anchor is allowed past the radius of a disc
    // sized 1.3× the total disc area. Prevents weakly-coupled clusters
    // from drifting to "infinity" — FR's equilibrium for a singly-tied
    // anchor against N other repulsors lands at ~N^(1/3)·k, which is
    // way past the area budget for big graphs.
    let canvas_r = (total_area * 1.3 / std::f32::consts::PI).sqrt();

    // Per-anchor max coupling normalization — same trick as the
    // spectral relaxation: each anchor's strongest tie is treated as
    // full-strength, regardless of how it compares to other clusters'
    // ties.
    let per_anchor_max: Vec<f32> = (0..n)
        .map(|i| {
            couplings[i]
                .iter()
                .map(|(_, w)| *w)
                .fold(0.0_f32, f32::max)
                .max(1.0)
        })
        .collect();

    // Initial per-iteration step cap — cooled to ~0 over ITERS so the
    // system settles instead of oscillating.
    let initial_step = k;

    for iter in 0..ITERS {
        let t = initial_step * (1.0 - (iter as f32) / (ITERS as f32));
        let mut forces = vec![(0.0_f32, 0.0_f32); n];

        // Repulsion between every pair (`F_r = k² / d`). Falls off
        // quickly, so a distant anchor's repulsion barely reaches.
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = centers[j].0 - centers[i].0;
                let dy = centers[j].1 - centers[i].1;
                let d = dx.hypot(dy).max(0.1);
                let f_r = k * k / d;
                let ux = dx / d;
                let uy = dy / d;
                forces[i].0 -= ux * f_r;
                forces[i].1 -= uy * f_r;
                forces[j].0 += ux * f_r;
                forces[j].1 += uy * f_r;
            }
        }

        // Spring attraction along couplings (`F_a = d² / k`). Scales
        // with coupling weight (sqrt-normalized) so a cluster's
        // dominant tie always wins over far-away repulsion.
        for i in 0..n {
            for &(j, w) in &couplings[i] {
                if j <= i {
                    continue;
                }
                let dx = centers[j].0 - centers[i].0;
                let dy = centers[j].1 - centers[i].1;
                let d = dx.hypot(dy).max(0.1);
                let norm = (w / per_anchor_max[i]).max(w / per_anchor_max[j]).sqrt();
                let f_a = d * d / k * norm;
                let ux = dx / d;
                let uy = dy / d;
                forces[i].0 += ux * f_a;
                forces[i].1 += uy * f_a;
                forces[j].0 -= ux * f_a;
                forces[j].1 -= uy * f_a;
            }
        }

        // Gravity toward the origin. Without it, weakly-coupled anchors
        // (1–2 ties) drift to the border because the cumulative
        // repulsion from every other anchor outweighs their small
        // attraction budget. Hooke-like pull `F_g = gravity · pos`
        // grows with distance so far outliers get yanked back the
        // hardest while well-positioned interior nodes feel almost
        // nothing.
        for i in 0..n {
            forces[i].0 -= GRAVITY * centers[i].0;
            forces[i].1 -= GRAVITY * centers[i].1;
        }

        // Apply, capping each anchor's step at the current temperature.
        for i in 0..n {
            let mag = forces[i].0.hypot(forces[i].1).max(1e-9);
            let cap = t.min(mag);
            centers[i].0 += forces[i].0 / mag * cap;
            centers[i].1 += forces[i].1 / mag * cap;
            // Hard clamp inside the canvas disc.
            let dist = centers[i].0.hypot(centers[i].1);
            if dist > canvas_r {
                let shrink = canvas_r / dist;
                centers[i].0 *= shrink;
                centers[i].1 *= shrink;
            }
        }
    }
}

/// `Noack`'s `LinLog` model. Distinct from FR / spectral / stress because
/// of the force shapes:
///
/// - **Attraction**: `F_a = norm(w) · base` — constant per edge,
///   independent of distance. A weakly-coupled pair feels the same
///   pull at any separation, so it can always close the gap rather
///   than giving up beyond some critical distance.
/// - **Repulsion**: `F_r = β · (rᵢ + rⱼ) / d` — logarithmic energy,
///   falls off as `1/d`. Distant clusters barely push each other; only
///   crowded neighborhoods spread out.
///
/// Result: communities collapse tightly and the layout is dominated by
/// who-connects-to-whom rather than the centroid-of-all-consumers
/// problem that traps FR with hub-and-spoke graphs.
#[expect(
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    reason = "indices are bounded by n; n well below 2^23"
)]
pub(crate) fn linlog_layout(
    centers: &mut [(f32, f32)],
    radii: &[f32],
    couplings: &[Vec<(usize, f32)>],
) {
    const ITERS: usize = 400;
    const GRAVITY: f32 = 0.04;
    let n = centers.len();
    if n < 2 {
        return;
    }

    let avg_r: f32 = radii.iter().sum::<f32>() / n as f32;
    let total_area: f32 = radii
        .iter()
        .map(|r| (r + ANCHOR_GAP).powi(2) * std::f32::consts::PI)
        .sum();
    let k = (total_area / n as f32).sqrt().max(avg_r * 2.5);
    let canvas_r = (total_area * 1.3 / std::f32::consts::PI).sqrt();

    // Per-anchor max coupling — same per-anchor normalization trick so
    // singly-coupled anchors get full pull strength on their one tie.
    let per_anchor_max: Vec<f32> = (0..n)
        .map(|i| {
            couplings[i]
                .iter()
                .map(|(_, w)| *w)
                .fold(0.0_f32, f32::max)
                .max(1.0)
        })
        .collect();

    // Attraction base is chosen so a single normalized coupling at
    // average disc-pair distance dominates the cumulative 1/d
    // repulsion. Concretely the per-anchor repulsion sum at canvas
    // scale is roughly Σ (rᵢ+rⱼ)/d ≈ N · 2·avg_r / canvas_r; the
    // attraction needs to exceed that for a singly-coupled pair to
    // close.
    let attract_base = k * 0.6;
    let repel_base = avg_r + ANCHOR_GAP;

    let initial_step = k * 0.8;
    for iter in 0..ITERS {
        let t = initial_step * (1.0 - (iter as f32) / (ITERS as f32));
        let mut forces = vec![(0.0_f32, 0.0_f32); n];

        // Logarithmic repulsion (force ∝ 1/d).
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = centers[j].0 - centers[i].0;
                let dy = centers[j].1 - centers[i].1;
                let d = dx.hypot(dy).max(0.1);
                let f_r = repel_base * (radii[i] + radii[j]) / (d * (avg_r + ANCHOR_GAP));
                let ux = dx / d;
                let uy = dy / d;
                forces[i].0 -= ux * f_r;
                forces[i].1 -= uy * f_r;
                forces[j].0 += ux * f_r;
                forces[j].1 += uy * f_r;
            }
        }

        // Constant per-edge attraction. The pull magnitude is
        // independent of how far apart the pair currently is.
        for i in 0..n {
            for &(j, w) in &couplings[i] {
                if j <= i {
                    continue;
                }
                let dx = centers[j].0 - centers[i].0;
                let dy = centers[j].1 - centers[i].1;
                let d = dx.hypot(dy).max(0.1);
                let norm = (w / per_anchor_max[i]).max(w / per_anchor_max[j]).sqrt();
                let f_a = norm * attract_base;
                let ux = dx / d;
                let uy = dy / d;
                forces[i].0 += ux * f_a;
                forces[i].1 += uy * f_a;
                forces[j].0 -= ux * f_a;
                forces[j].1 -= uy * f_a;
            }
        }

        // Mild gravity + canvas clamp as belt-and-braces against the
        // log-repulsion-only-falls-off-slowly tendency to spread out.
        for i in 0..n {
            forces[i].0 -= GRAVITY * centers[i].0;
            forces[i].1 -= GRAVITY * centers[i].1;
        }

        for i in 0..n {
            let mag = forces[i].0.hypot(forces[i].1).max(1e-9);
            let cap = t.min(mag);
            centers[i].0 += forces[i].0 / mag * cap;
            centers[i].1 += forces[i].1 / mag * cap;
            let dist = centers[i].0.hypot(centers[i].1);
            if dist > canvas_r {
                let shrink = canvas_r / dist;
                centers[i].0 *= shrink;
                centers[i].1 *= shrink;
            }
        }
    }
}

/// Stress majorization: drive every pair `(i, j)` to a Euclidean
/// distance proportional to its graph-theoretic shortest path on the
/// anchor super-graph. Each iteration solves the per-coordinate
/// majorized update in closed form (weighted average — see the Gansner
/// et al. "Graph drawing by stress majorization" paper). Seeded from
/// the spectral embedding so global structure is right from step 0
/// and only the metric gets refined.
///
/// Graph edge length: `1/√weight` — strongly-coupled pairs get short
/// graph distance, so they end up tight in the embedding. Disconnected
/// pairs are clamped to (graph diameter × 1.5) so they still repel
/// rather than landing on top of each other.
#[expect(
    clippy::indexing_slicing,
    clippy::cast_precision_loss,
    reason = "indices are bounded by n; n well below 2^23"
)]
pub(crate) fn stress_layout(
    centers: &mut [(f32, f32)],
    radii: &[f32],
    couplings: &[Vec<(usize, f32)>],
) {
    const ITERS: usize = 250;
    let n = centers.len();
    if n < 2 {
        return;
    }

    let dist = all_pairs_dijkstra(couplings, n);

    // Convert hop-count graph distances to canvas-unit ideal distances.
    let avg_r: f32 = radii.iter().sum::<f32>() / n as f32;
    let unit = (avg_r + ANCHOR_GAP) * 2.0;
    let target: Vec<f32> = dist.iter().map(|d| d * unit).collect();

    let mut next: Vec<(f32, f32)> = vec![(0.0, 0.0); n];
    for _ in 0..ITERS {
        for i in 0..n {
            let mut weight_sum = 0.0_f32;
            let mut acc_x = 0.0_f32;
            let mut acc_y = 0.0_f32;
            for j in 0..n {
                if j == i {
                    continue;
                }
                let d_ij = target[i * n + j];
                if d_ij <= 0.0 {
                    continue;
                }
                let w = 1.0 / (d_ij * d_ij);
                let dx = centers[i].0 - centers[j].0;
                let dy = centers[i].1 - centers[j].1;
                let euc = dx.hypot(dy).max(0.001);
                let factor = d_ij / euc;
                weight_sum += w;
                acc_x += w * (centers[j].0 + factor * (centers[i].0 - centers[j].0));
                acc_y += w * (centers[j].1 + factor * (centers[i].1 - centers[j].1));
            }
            next[i] = if weight_sum > 0.0 {
                (acc_x / weight_sum, acc_y / weight_sum)
            } else {
                centers[i]
            };
        }
        centers.copy_from_slice(&next);
    }
}

/// All-pairs shortest paths via repeated O(N²) Dijkstra. For 366
/// anchors this is ~50M ops total — sub-second in release, ~3 s in
/// debug. Returns a flat `n*n` matrix (`dist[i*n + j]`). Edge length
/// is `1/√weight` so the strongest tie has the shortest hop.
#[expect(
    clippy::indexing_slicing,
    reason = "indices come from inner loops bounded by n / couplings[i].len()"
)]
pub(crate) fn all_pairs_dijkstra(couplings: &[Vec<(usize, f32)>], n: usize) -> Vec<f32> {
    let mut out = vec![f32::INFINITY; n * n];
    for src in 0..n {
        let mut visited = vec![false; n];
        let mut d = vec![f32::INFINITY; n];
        d[src] = 0.0;
        for _ in 0..n {
            // Naive O(N) extract-min. With 366 nodes a binary heap is
            // overkill; the constant factor of the simple loop is
            // smaller in practice.
            let mut u = usize::MAX;
            let mut best = f32::INFINITY;
            for i in 0..n {
                if !visited[i] && d[i] < best {
                    best = d[i];
                    u = i;
                }
            }
            if u == usize::MAX {
                break;
            }
            visited[u] = true;
            for &(v, w) in &couplings[u] {
                let edge_len = 1.0 / w.sqrt().max(0.01);
                let alt = d[u] + edge_len;
                if alt < d[v] {
                    d[v] = alt;
                }
            }
        }
        for i in 0..n {
            out[src * n + i] = d[i];
        }
    }
    // Replace +∞ (disconnected pairs) with diameter * 1.5 so the
    // stress update still pushes them apart without overflowing.
    let mut max_finite = 0.0_f32;
    for v in &out {
        if v.is_finite() && *v > max_finite {
            max_finite = *v;
        }
    }
    let fallback = max_finite.mul_add(1.5, 1.0);
    for v in &mut out {
        if !v.is_finite() {
            *v = fallback;
        }
    }
    out
}

/// Final cleanup pass — guarantees the property the previous greedy
/// pack used to give us: no two discs overlap. Iterates until stable
/// or `MAX_PASSES` exhausted.
#[expect(
    clippy::indexing_slicing,
    reason = "indices come from the parallel radii / centers vectors"
)]
pub(crate) fn enforce_non_overlap(centers: &mut [(f32, f32)], radii: &[f32]) {
    const MAX_PASSES: usize = 200;
    let n = centers.len();
    for _ in 0..MAX_PASSES {
        let mut moved = false;
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = centers[j].0 - centers[i].0;
                let dy = centers[j].1 - centers[i].1;
                let d = dx.hypot(dy).max(0.001);
                let min_d = radii[i] + radii[j] + ANCHOR_GAP;
                if d < min_d {
                    let push = (min_d - d) * 0.5 + 0.5;
                    let ux = dx / d;
                    let uy = dy / d;
                    centers[i].0 -= ux * push;
                    centers[i].1 -= uy * push;
                    centers[j].0 += ux * push;
                    centers[j].1 += uy * push;
                    moved = true;
                }
            }
        }
        if !moved {
            break;
        }
    }
}

/// Place node `i` of an `n`-node anchor at an offset within a disc
/// of radius `disc_r`. Fermat spiral — `r ∝ sqrt(i / n)` — gives
/// uniform-density packing without a center hotspot.
pub(crate) fn node_offset_in_disc(i: usize, n: usize, disc_r: f32) -> (f32, f32) {
    if n <= 1 {
        // Single-node anchor — place at center.
        return (0.0, 0.0);
    }
    // Reserve a small inner blank ring so the densest connections
    // don't all converge to a single point.
    #[expect(clippy::cast_precision_loss, reason = "n well below 2^23")]
    let frac = (i as f32 + 0.5) / (n as f32);
    let r = disc_r * (0.05 + 0.90 * frac.sqrt());
    #[expect(clippy::cast_precision_loss, reason = "i well below 2^23")]
    let theta = (i as f32) * GOLDEN_ANGLE;
    (r * theta.cos(), r * theta.sin())
}
