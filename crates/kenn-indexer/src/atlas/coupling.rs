//! Package-level coupling: who depends on whom, how heavily, and by which
//! relations — plus the role classification `index.md` groups by.
//!
//! Extracted so the atlas producer and the `packages` query compute coupling the
//! SAME way. They start from different inputs — the producer from the in-memory
//! graph mid-index, the query from `aggregate_nodes`/`aggregate_edges` rows of a
//! published snapshot — but the RULES here (what counts as a dependency, how
//! relations split, the render caps, where a role's boundaries sit) must be one
//! implementation. Two copies of a threshold is how a CLI and a document start
//! disagreeing about the same repo.

use std::collections::HashMap;

use super::model::{Coupling, Role};
use super::okf;

/// Directed cross-anchor weights, keyed `(src_anchor, dst_anchor)` → relation →
/// weight. Built once per graph and read from both directions.
pub type PairWeights<'a> = HashMap<(&'a str, &'a str), HashMap<&'static str, u64>>;

pub const MAX_DEPS: usize = 8;
/// See [`Direction::cap`] — dependents are a popularity measure, not a design
/// choice, so the incoming direction shows far more before it truncates.
pub const MAX_DEPENDENTS: usize = 24;

/// Instability bounds separating [`Role::Provider`] / [`Role::Layer`] /
/// [`Role::Consumer`]. `I = out / (out + in)` — Martin's instability, over
/// aggregate-edge weight rather than class count.
///
/// A quarter at each end rather than thirds: on every repo measured the
/// distribution is bimodal, clustering near 0 (foundations) and near 1 (leaves),
/// with genuinely mixed packages sparse in between. Widening the middle would
/// swallow packages that are plainly one or the other.
pub const PROVIDER_MAX_I: f64 = 0.25;
pub const CONSUMER_MIN_I: f64 = 0.75;

/// Classify a package by how much it is depended on versus how much it depends.
///
/// Tests first: a test-dominant package is not a load-bearing part of the
/// architecture whatever its coupling looks like, and on a large solution these
/// are a third of the package list. Then isolation — no coupling either way
/// means instability is undefined, not zero, and calling such a package a
/// "provider" (`out == 0`) would be exactly backwards.
#[must_use]
pub fn classify(test: bool, in_w: u64, out_w: u64) -> Role {
    if test {
        return Role::Tests;
    }
    let total = in_w + out_w;
    if total == 0 {
        return Role::Isolated;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "edge weights are far below f64's exact-integer range; the ratio only needs to land in a quartile"
    )]
    let instability = out_w as f64 / total as f64;
    if instability <= PROVIDER_MAX_I {
        Role::Provider
    } else if instability >= CONSUMER_MIN_I {
        Role::Consumer
    } else {
        Role::Layer
    }
}

/// Which side of a coupling pair an anchor sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// `anchor → other` — what it depends on.
    Out,
    /// `other → anchor` — what depends on it.
    In,
}

impl Direction {
    /// How many rows this direction renders.
    ///
    /// Asymmetric on purpose. Outgoing dependencies are a design CHOICE a
    /// package makes, and a package with 40 of them has a problem the top 8
    /// already reveals. Incoming ones are a POPULARITY measure it does not
    /// control: on a real 125-package solution one utility package had 100
    /// dependents, so the shared cap showed 8% of the truth and read as the
    /// whole list.
    #[must_use]
    pub const fn cap(self) -> usize {
        match self {
            Self::Out => MAX_DEPS,
            Self::In => MAX_DEPENDENTS,
        }
    }
}

/// One anchor's coupling in one direction: the rows to render, plus the pre-cap
/// facts a caller must not derive from them.
///
/// `weight` is the summed weight over EVERY coupling, not just `rows`. Role
/// classification depends on it: `rows` is truncated for display, so summing it
/// understates a popular package's inbound weight and can move it between roles
/// — which is exactly what the cap must never do.
#[derive(Debug, Clone)]
pub struct Couplings {
    /// Capped for display, heaviest first.
    pub rows: Vec<Coupling>,
    /// Coupled packages BEFORE the cap.
    pub total: u64,
    /// Summed weight BEFORE the cap.
    pub weight: u64,
}

/// Project the cross-anchor pair map into one anchor's couplings for `dir`,
/// heaviest first. The returned [`Couplings`] carries the pre-cap count AND the
/// pre-cap weight, so the renderer can say what it dropped instead of
/// truncating silently, and classification never sees a truncated total.
///
/// Ties break on the concept id so the bundle stays byte-identical across a
/// re-index of unchanged code — the same determinism contract the rest of the
/// atlas holds to (design R3-C).
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "both callers pass the std-default HashMap; generalizing over BuildHasher only adds noise"
)]
pub fn couplings(
    pair_w: &PairWeights<'_>,
    anchor_lang: &HashMap<&str, String>,
    anchor: &str,
    dir: Direction,
) -> Couplings {
    let mut out: Vec<Coupling> = pair_w
        .iter()
        .filter_map(|(&(src, dst), rels)| {
            let other = match dir {
                Direction::Out if src == anchor => dst,
                Direction::In if dst == anchor => src,
                _ => return None,
            };
            let mut relations: Vec<(String, u64)> =
                rels.iter().map(|(&k, &v)| (k.to_string(), v)).collect();
            relations.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            Some(Coupling {
                concept_id: okf::concept_id(
                    anchor_lang.get(other).map_or("", String::as_str),
                    other,
                ),
                title: other.to_string(),
                weight: relations.iter().map(|(_, w)| w).sum(),
                relations,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.weight
            .cmp(&a.weight)
            .then_with(|| a.concept_id.cmp(&b.concept_id))
    });
    let total = out.len() as u64;
    let weight = out.iter().map(|c| c.weight).sum();
    out.truncate(dir.cap());
    Couplings {
        rows: out,
        total,
        weight,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests and isolation are checked BEFORE instability, both for a reason. A
    /// test-dominant package is not load-bearing architecture whatever its
    /// coupling looks like. And a package with no coupling either way has
    /// UNDEFINED instability, not zero — falling through would file it under
    /// Providers, the exact opposite of the truth. Mutation-checked: dropping
    /// the `total == 0` guard classifies an isolated package as a Provider.
    #[test]
    fn role_is_classified_by_dependency_direction() {
        // Depended on 100, depends on 1 → the foundation.
        assert_eq!(classify(false, 100, 1), Role::Provider);
        // The mirror: an app nothing depends on.
        assert_eq!(classify(false, 0, 50), Role::Consumer);
        // Genuinely both.
        assert_eq!(classify(false, 50, 50), Role::Layer);
        // Test-dominance wins over any coupling shape.
        assert_eq!(classify(true, 100, 1), Role::Tests);
        // No coupling at all is its own answer, never Provider.
        assert_eq!(classify(false, 0, 0), Role::Isolated);
    }

    /// The quartile boundaries are inclusive on the provider/consumer side, so a
    /// package sitting exactly on one is not silently swallowed by Layer.
    #[test]
    fn role_boundaries_are_inclusive() {
        assert_eq!(classify(false, 75, 25), Role::Provider, "I = 0.25");
        assert_eq!(classify(false, 25, 75), Role::Consumer, "I = 0.75");
        assert_eq!(classify(false, 74, 26), Role::Layer, "just inside");
    }

    /// The render cap must NOT move a package between roles — the defect this
    /// struct exists to prevent.
    ///
    /// 40 dependents at weight 10 (in = 400) and one dependency at 100
    /// (out = 100). Instability over the FULL sets is 100/500 = 0.20 → Provider.
    /// Summing the RENDERED rows instead sees only 24 dependents (in = 240), so
    /// instability reads 100/340 = 0.29 and the package is filed under Layer —
    /// a package the whole repo rests on, described as a middle layer.
    #[test]
    fn the_render_cap_does_not_change_a_role() {
        let mut pair_w: PairWeights<'_> = HashMap::new();
        for i in 0..40u32 {
            let src: &'static str = Box::leak(format!("dep{i:02}").into_boxed_str());
            pair_w.insert((src, "core"), [("calls", 10u64)].into_iter().collect());
        }
        pair_w.insert(("core", "util"), [("calls", 100u64)].into_iter().collect());
        let langs: HashMap<&str, String> = HashMap::new();

        let used_by = couplings(&pair_w, &langs, "core", Direction::In);
        let deps = couplings(&pair_w, &langs, "core", Direction::Out);

        assert_eq!(used_by.total, 40, "40 dependents before the cap");
        assert_eq!(used_by.rows.len(), MAX_DEPENDENTS, "capped for display");
        assert_eq!(used_by.weight, 400, "pre-cap weight survives the cap");

        assert_eq!(
            classify(false, used_by.weight, deps.weight),
            Role::Provider,
            "the honest weights: 400 in, 100 out"
        );
        let truncated: u64 = used_by.rows.iter().map(|c| c.weight).sum();
        assert_eq!(truncated, 240, "the rendered rows hold 24 of the 40");
        assert_eq!(
            classify(false, truncated, deps.weight),
            Role::Layer,
            "summing the rendered rows demotes a Provider to a Layer — the bug"
        );
    }
}
