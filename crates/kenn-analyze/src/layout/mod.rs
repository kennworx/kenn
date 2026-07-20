//! Server-side static layout for the aggregated graph.
//!
//! Why server-side: client-side force simulators (cosmos, cytoscape
//! cose/fcose, vis.js stabilization) either pulse forever or hang on
//! the biggest workspace. Computing positions once in Rust and
//! shipping them as JSON lets the browser just paint — no sim, no
//! animation, no surprise CPU.
//!
//! Algorithm: circle-pack the anchors, then place nodes inside each
//! anchor's disc.
//!
//! 1. Each anchor gets a disc whose radius scales with sqrt(node
//!    count) — area proportional to nodes, so a 500-node anchor
//!    occupies ~5× the area of a 100-node anchor.
//! 2. Discs are packed greedily on a sunflower spiral keyed to the
//!    running cumulative radius, so big and small anchors interleave
//!    naturally without one dominating the center.
//! 3. Within each disc, nodes are placed deterministically on a
//!    Fermat spiral; no node sits at the dead center (which used to
//!    cause an unintentional hub-and-spoke when the heaviest node
//!    had many cross-anchor edges).
//!
//! Cost: O(N). On the largest validation workspace (12k nodes / 121k
//! edges) this runs in milliseconds. Output is byte-deterministic
//! across runs: anchor iteration order is sorted by (size desc, name
//! asc), nodes within an anchor by (`weighted_degree` desc, `short_id`
//! asc).
//!
//! Module layout:
//! - [`api`] — algorithm/result types, constants, and the `compute` entry.
//! - [`seed`] — coupling construction + spectral/sunflower seeding.
//! - [`relax`] — force/linlog/stress relaxation + node placement.

mod api;
mod relax;
mod seed;

#[cfg(test)]
#[expect(
    clippy::get_unwrap,
    clippy::cast_precision_loss,
    reason = "test fixtures are bounded and panicking on bad input is the test failure mode"
)]
mod tests;

pub use api::{compute, Layout, LayoutAlgo};

pub(crate) use api::{ANCHOR_GAP, GOLDEN_ANGLE};
pub(crate) use relax::{
    enforce_non_overlap, force_layout, linlog_layout, node_offset_in_disc, relax_with_couplings,
    stress_layout,
};
pub(crate) use seed::{build_anchor_couplings, compute_weighted_degrees, pack_anchor_centers};

#[cfg(test)]
pub(crate) use relax::all_pairs_dijkstra;
