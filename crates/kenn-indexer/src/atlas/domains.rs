//! Domain-axis SELECTION: which flat-Louvain communities are real cross-package
//! domains, which packages earned a place in each span, and how their members
//! rank.
//!
//! Extracted so the atlas producer and the domains query select the SAME
//! domains. They start from different inputs — the producer from the in-memory
//! graph mid-index, the query from a published snapshot's rows — so everything
//! here takes input-agnostic projections (anchor names, node ids, weights) and
//! each caller projects its own types in. Same pattern, and same reason, as
//! [`super::coupling`]: two copies of a floor is how a CLI and a document start
//! disagreeing about the same repo.
//!
//! Selection only. The render caps (`MAX_DOMAINS`, `MAX_DOMAIN_PKGS`,
//! `MAX_CENTRAL`) and the concept-id slugs live in the producer: a cap is
//! presentation policy for a page with a reader, and must never bound a query.

use std::collections::{HashMap, HashSet};

use kenn_model::ShortId;

/// Eligible members a community needs before it can be a domain at all.
pub const MIN_DOMAIN_SIZE: usize = 4;

/// Members a package needs in a community before it can be part of the span.
/// One straggler symbol is a reference into the domain, not membership of it.
pub const MIN_PKG_MEMBERS: u64 = 2;

/// Distinct first-party cross-package edges a domain's span must rest on. A
/// single reference between two packages is a mention, not a shared domain.
pub const MIN_DOMAIN_LINKS: u64 = 2;

/// Languages whose symbols can seed a domain. The atlas maps CODE structure;
/// a markdown note or a stylesheet is a document, and letting one hub a domain
/// titles the cluster by prose.
#[must_use]
pub fn is_code_lang(db_name: &str) -> bool {
    matches!(
        db_name,
        "rust" | "typescript" | "csharp" | "go" | "python" | "swift"
    )
}

/// The facts an aggregate node contributes to domain eligibility, projected by
/// the caller from its own `Record` or `Row` type.
#[derive(Debug, Clone, Copy)]
pub struct NodeFacts<'a> {
    pub id: ShortId,
    pub language: &'a str,
    pub kind: &'a str,
    pub name: &'a str,
    pub external: bool,
    pub test: bool,
    /// The node's primary definition file lies under an example/sample/demo/
    /// fixtures directory. Read straight off the aggregate node — the
    /// aggregation pass evaluates it once and persists it, precisely because a
    /// query over a published snapshot sees no paths and would otherwise have
    /// to guess. It guessed `false`, and reported a domain the atlas did not.
    pub example: bool,
}

/// Whether a node may SEED a domain: first-party, non-test, non-example, named,
/// a real type rather than a container, and in a code language.
///
/// Shared because the two callers kept drifting apart one predicate at a time —
/// a query that forgot the language filter titled a domain with a markdown note,
/// and one that forgot the container filter would seed domains with modules.
#[must_use]
pub fn is_domain_eligible(n: &NodeFacts<'_>, anchored: bool) -> bool {
    anchored
        && !n.external
        && !n.test
        && !n.example
        && !n.name.is_empty()
        && !matches!(n.kind, "module" | "namespace" | "package")
        && is_code_lang(n.language)
}

/// One aggregate edge, projected: endpoints plus weight. The caller flattens its
/// own `AggregateEdgeRecord` / `AggregateEdgeRow` into this.
#[derive(Debug, Clone, Copy)]
pub struct Edge {
    pub src: ShortId,
    pub dst: ShortId,
    pub weight: u32,
}

/// A community's earned span: `(package, members, links)` rows, heaviest first.
pub type Span<'a> = Vec<(&'a str, u64, u64)>;

/// The outcome of [`decide_span`] — the span rows, plus the packages a domain's
/// members are restricted to (`None` = keep every member, the intra-package
/// case).
pub type SpanDecision<'a> = (Span<'a>, Option<HashSet<&'a str>>);

/// One community that earned domain status.
#[derive(Debug, Clone)]
pub struct SelectedDomain<'a> {
    pub community_id: u32,
    /// Members restricted to the earned span, ranked by INTRA-domain weighted
    /// degree (the hub first), ties broken by symbol name then id.
    pub ranked: Vec<ShortId>,
    /// The earned span, heaviest first and UNCAPPED: `(package, members, links)`
    /// where `links` is the package's intra-domain coupling to the rest of the
    /// span. Callers truncate for display; a query must not.
    pub packages: Span<'a>,
}

/// The packages a community genuinely spans: those with at least
/// [`MIN_PKG_MEMBERS`] members AND a first-party edge to another such package.
///
/// `pairs` is the community's cross-package edge counts (unordered package key →
/// count). A package earns the span only through edges to ANOTHER candidate; any
/// candidate↔candidate edge supports BOTH its endpoints, so the packages that
/// appear here are exactly the supported set and each row's link count is its
/// coupling to the rest of the span. Fewer than two rows means the community is
/// one package dressed as a span, or a set glued only through shared external
/// types — neither is a domain.
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "both callers pass the std-default HashMap; generalizing over BuildHasher only adds noise"
)]
pub fn supported_span<'a>(
    pkg_members: &HashMap<&'a str, u64>,
    pairs: &HashMap<(&'a str, &'a str), u64>,
) -> Span<'a> {
    let is_candidate = |p: &str| pkg_members.get(p).is_some_and(|&c| c >= MIN_PKG_MEMBERS);
    let mut links: HashMap<&'a str, u64> = HashMap::new();
    for (&(a, b), &n) in pairs {
        if is_candidate(a) && is_candidate(b) {
            *links.entry(a).or_default() += n;
            *links.entry(b).or_default() += n;
        }
    }
    let mut out: Span<'a> = links
        .into_iter()
        .map(|(p, l)| (p, pkg_members[p], l))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    out
}

/// First-party cross-package coupling INSIDE each community: an edge whose
/// endpoints are two eligible members of the same community in different
/// packages, counted per unordered package pair. This is the evidence
/// [`supported_span`] requires — the external hubs that glued the raw community
/// together contribute nothing (they are not eligible members, so `node_comm`
/// skips them).
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "both callers pass the std-default HashMap; generalizing over BuildHasher only adds noise"
)]
pub fn community_pair_links<'a>(
    node_comm: &HashMap<ShortId, u32>,
    node_anchor: &HashMap<ShortId, &'a str>,
    edges: &[Edge],
) -> HashMap<u32, HashMap<(&'a str, &'a str), u64>> {
    let mut comm_pairs: HashMap<u32, HashMap<(&str, &str), u64>> = HashMap::new();
    // One REFERENCE counts once. The aggregate graph splits a pair across one
    // row per edge kind, so a symbol that both names a type in another package
    // and calls a method on it yields `type_use` + `calls` — two rows for one
    // relationship, which alone clears MIN_DOMAIN_LINKS and makes the floor a
    // no-op. Counting distinct node pairs restores what the floor claims: a
    // single reference between two packages is a mention, not a shared domain.
    let mut counted: HashSet<(ShortId, ShortId)> = HashSet::new();
    for e in edges {
        let (Some(&cs), Some(&cd)) = (node_comm.get(&e.src), node_comm.get(&e.dst)) else {
            continue;
        };
        if cs != cd {
            continue;
        }
        let (Some(&pa), Some(&pb)) = (node_anchor.get(&e.src), node_anchor.get(&e.dst)) else {
            continue;
        };
        if pa == pb {
            continue;
        }
        if !counted.insert((e.src, e.dst)) {
            continue; // another kind of the same reference
        }
        let key = if pa < pb { (pa, pb) } else { (pb, pa) };
        *comm_pairs.entry(cs).or_default().entry(key).or_default() += 1;
    }
    comm_pairs
}

/// Decide a community's span: the packages it genuinely spans, and the set its
/// members are restricted to (`None` = keep every member, the intra-package
/// case). `None` overall means the community is not a domain.
///
/// A cross-package domain needs two edge-supported packages AND more than a
/// single cross-package reference — [`MIN_DOMAIN_LINKS`] distinct edges, where
/// the per-package link sum double-counts each edge (both endpoints). Failing
/// that, a single-package community is kept only for a single-dominant repo (a
/// monolithic library's internal cluster the package axis can't surface).
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "both callers pass the std-default HashMap; generalizing over BuildHasher only adds noise"
)]
pub fn decide_span<'a>(
    pkg_members: &HashMap<&'a str, u64>,
    pairs: &HashMap<(&'a str, &'a str), u64>,
    single_dominant: bool,
) -> Option<SpanDecision<'a>> {
    let supported = supported_span(pkg_members, pairs);
    let distinct_edges = supported.iter().map(|&(_, _, l)| l).sum::<u64>() / 2;
    if supported.len() >= 2 && distinct_edges >= MIN_DOMAIN_LINKS {
        let keep: HashSet<&str> = supported.iter().map(|&(p, ..)| p).collect();
        Some((supported, Some(keep)))
    } else if single_dominant && pkg_members.len() == 1 {
        // One package means no cross-package span, hence no links.
        let (&a, &m) = pkg_members.iter().next()?;
        Some((vec![(a, m, 0)], None))
    } else {
        // Glued only through externals, or a single package dressed as a span —
        // not a domain the package axis doesn't already cover.
        None
    }
}

/// Select every community that earns domain status, in ascending community-id
/// order (the caller re-sorts for display).
///
/// `keep` is the candidate community set (cross-anchor, or any community for a
/// single-dominant repo). `eligible` is the code+anchored node set — the same
/// one package centrality ranks, so a container/external/content node never
/// seeds a domain. `symbol_name` supplies the ranking tie-break and the hub
/// name: a community whose hub has no name is dropped, because an unnameable
/// concept is not a useful one.
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "both callers pass the std-default HashMap; generalizing over BuildHasher only adds noise"
)]
pub fn select_domains<'a>(
    keep: &HashSet<u32>,
    membership: &[(ShortId, u32)],
    eligible: &HashSet<ShortId>,
    edges: &[Edge],
    node_anchor: &HashMap<ShortId, &'a str>,
    symbol_name: &HashMap<ShortId, &str>,
    single_dominant: bool,
) -> Vec<SelectedDomain<'a>> {
    if keep.is_empty() {
        return Vec::new();
    }

    // Members per kept community, restricted to code + anchored nodes.
    let mut members: HashMap<u32, Vec<ShortId>> = HashMap::new();
    let mut node_comm: HashMap<ShortId, u32> = HashMap::new();
    for &(short_id, community_id) in membership {
        if keep.contains(&community_id) && eligible.contains(&short_id) {
            members.entry(community_id).or_default().push(short_id);
            node_comm.insert(short_id, community_id);
        }
    }

    let comm_pairs = community_pair_links(&node_comm, node_anchor, edges);
    let intra_degree = intra_degrees(&node_comm, edges);

    let name_of = |s: ShortId| symbol_name.get(&s).copied().unwrap_or("");
    let no_pairs: HashMap<(&str, &str), u64> = HashMap::new();
    let mut out: Vec<SelectedDomain<'a>> = Vec::new();
    let mut cids: Vec<u32> = members.keys().copied().collect();
    cids.sort_unstable();
    for cid in cids {
        let Some(nodes) = members.get(&cid) else {
            continue;
        };
        if nodes.len() < MIN_DOMAIN_SIZE {
            continue; // enough code members after filtering to be a real domain
        }

        // Per-package member counts across every eligible member.
        let mut pkg_members: HashMap<&str, u64> = HashMap::new();
        for &s in nodes {
            if let Some(&a) = node_anchor.get(&s) {
                *pkg_members.entry(a).or_default() += 1;
            }
        }

        let Some((packages, keep_pkgs)) = decide_span(
            &pkg_members,
            comm_pairs.get(&cid).unwrap_or(&no_pairs),
            single_dominant,
        ) else {
            continue;
        };

        // Rank members, restricted to the spanned packages, so the size and the
        // central list describe the honest cluster and not the raw community.
        let mut ranked: Vec<ShortId> = nodes
            .iter()
            .copied()
            .filter(|s| {
                keep_pkgs
                    .as_ref()
                    .is_none_or(|keep| node_anchor.get(s).is_some_and(|a| keep.contains(a)))
            })
            .collect();
        // Degree, then name, then id. The id tie-break is what the hub (and so
        // the domain's title, concept id, and query handle) rests on when two
        // members tie on both — without it the hub follows membership scan order.
        ranked.sort_by(|&a, &b| {
            intra_degree
                .get(&b)
                .unwrap_or(&0)
                .cmp(intra_degree.get(&a).unwrap_or(&0))
                .then_with(|| name_of(a).cmp(name_of(b)))
                .then_with(|| a.cmp(&b))
        });
        if ranked.first().is_none_or(|&s| name_of(s).is_empty()) {
            continue; // no nameable hub → not a useful concept
        }

        out.push(SelectedDomain {
            community_id: cid,
            ranked,
            packages,
        });
    }
    out
}

/// Intra-domain weighted degree: Σ edge weight to OTHER members of the same
/// community.
///
/// The hub is the symbol most connected WITHIN the domain, not the one
/// most-referenced repo-wide — a ubiquitous value type (an enum used everywhere)
/// has enormous GLOBAL degree yet is peripheral to any single cluster, so ranking
/// by global degree titles a domain by that value type and buries the
/// service/type that actually organizes it.
fn intra_degrees(node_comm: &HashMap<ShortId, u32>, edges: &[Edge]) -> HashMap<ShortId, u64> {
    let mut intra: HashMap<ShortId, u64> = HashMap::new();
    for e in edges {
        if let (Some(&cs), Some(&cd)) = (node_comm.get(&e.src), node_comm.get(&e.dst)) {
            if cs == cd {
                *intra.entry(e.src).or_default() += u64::from(e.weight);
                *intra.entry(e.dst).or_default() += u64::from(e.weight);
            }
        }
    }
    intra
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The core span rule as a pure function. A package earns the span only by
    /// clearing the member floor AND linking to another package that also clears
    /// it; the link count returned is its coupling to the rest of the span.
    /// Mutation-checked: dropping the `>= MIN_PKG_MEMBERS` gate would admit the
    /// 1-member straggler `c`; dropping the candidate check on the OTHER endpoint
    /// would let an edge into `c` support `a`.
    #[test]
    fn supported_span_keeps_only_linked_substantial_packages() {
        let pkg_members: HashMap<&str, u64> = [("a", 3), ("b", 2), ("c", 1)].into_iter().collect();
        // a↔b is a candidate↔candidate edge (2 of them); a↔c touches a 1-member
        // straggler and must not count.
        let pairs: HashMap<(&str, &str), u64> =
            [(("a", "b"), 2), (("a", "c"), 5)].into_iter().collect();
        let span = supported_span(&pkg_members, &pairs);
        assert_eq!(
            span,
            vec![("a", 3, 2), ("b", 2, 2)],
            "only a and b span; c is a reference, not a member; links = a↔b edges"
        );
    }

    /// A community glued only through shared EXTERNAL types has no first-party
    /// intra-community edge, so no package earns the span — the rule that turned
    /// the raw Louvain output into an honest axis.
    #[test]
    fn supported_span_rejects_packages_with_no_first_party_edge() {
        let pkg_members: HashMap<&str, u64> = [("a", 5), ("b", 4)].into_iter().collect();
        assert!(
            supported_span(&pkg_members, &HashMap::new()).is_empty(),
            "no intra-community first-party edge → no span"
        );
    }
}
