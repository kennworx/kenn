//! `list_domains` — the domain axis of the graph, as a query.
//!
//! The atlas renders this as markdown at index time; this answers the same
//! question on demand. Both go through `kenn_indexer::atlas::domains`, so a
//! floor can never mean one thing in the document and another at the prompt.
//!
//! Reads the published snapshot's aggregate graph plus the persisted flat-Louvain
//! analysis; it never re-clusters. Only EARNED-span domains are returned — the
//! same subset the atlas renders, never the raw community count.
//!
//! Every eligibility fact comes off the aggregate node row, including `example`
//! — which is evaluated once at aggregation time precisely because this query
//! cannot see definition paths. While it was a producer-side path join, this
//! query had to pass `example: false` and reported a domain whose entire
//! cross-package span was carried by example binaries.

use std::collections::{HashMap, HashSet};

use kenn_indexer::atlas::domains;
use kenn_store::api::Reader;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::McpError;
use crate::types::ListResponse;

use super::{internal, ServerState};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ListDomainsArgs {
    /// Restrict to one domain — its hub symbol's `pub_id`, or its title. A title
    /// is a QUERY, not an identifier: when it matches more than one domain every
    /// match is returned rather than an error.
    #[serde(default)]
    pub domain: Option<String>,
    /// Rows per response and the continuation cursor. The axis is computed
    /// whole and ordered deterministically, so this is a plain offset walk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<crate::types::Pagination>,
}

/// One package a domain genuinely spans.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SpannedPackageView {
    pub package: String,
    /// The domain's members living in this package.
    pub members: u64,
    /// Intra-domain edges connecting it to the domain's OTHER spanned packages —
    /// the coupling that earned it the span.
    pub links: u64,
}

/// One of a domain's most central members — a resolvable handle to drill in.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DomainMemberView {
    /// The stable `pub_id` — feed it straight to `kenn get` / `kenn list`.
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DomainView {
    /// The hub symbol's `pub_id` — the domain's resolvable handle, and what you
    /// name to get its detail. ALWAYS serialized so every row carries the same
    /// fields and the listing stays a flat table. First column: it is the id a
    /// reader acts on.
    pub symbol: String,
    /// The hub symbol's name — the domain's title, as the atlas titles it.
    pub title: String,
    /// Members of the honest cluster (restricted to the earned span).
    pub size: u64,
    /// Packages the domain genuinely spans, and the cross-package links that
    /// earned that span. These are the counts the selection was derived from.
    pub packages_count: u64,
    pub links: u64,
    /// Populated only when a single `domain` was requested — the full listing
    /// would be quadratic and unreadable.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<SpannedPackageView>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub central: Vec<DomainMemberView>,
}

/// Central members shown for a single named domain — matches the atlas cap.
const CENTRAL_CAP: usize = 8;

/// One earned domain, resolved down to ids the async shell can look up.
#[derive(Debug, Clone)]
pub struct DomainSelection {
    /// The hub: the domain's highest intra-degree member, and its handle.
    pub hub: u32,
    /// Members restricted to the earned span, hub first.
    pub ranked: Vec<u32>,
    /// `(package, members, links)` — the earned span, uncapped.
    pub packages: Vec<(String, u64, u64)>,
}

/// The pure selection: project snapshot rows into the shared rule and return
/// each earned domain's hub, size, span, and ranked members.
///
/// Separated from the async shell so the rules are testable without a published
/// snapshot behind them.
#[must_use]
pub fn domain_selections(
    nodes: &[kenn_store::AggregateNodeRow],
    edges: &[kenn_store::AggregateEdgeRow],
    flat: &[kenn_store::AnalysisFlatCommunityRow],
    membership: &[kenn_store::AnalysisNodeMembershipRow],
) -> Vec<DomainSelection> {
    // Anchor per node, first-party + anchored code only — the same eligibility
    // the package axis uses, so a container/external node never seeds a domain.
    let node_anchor: HashMap<u32, &str> = nodes
        .iter()
        .filter(|n| !n.external && n.anchor_name != "<unanchored>")
        .map(|n| (n.id, n.anchor_name.as_str()))
        .collect();
    // One shared predicate over facts that all come off the row, so this can't
    // drift from the producer the way it did when the language filter was
    // missing here and a markdown note titled a domain — or when `example` was
    // a producer-only path join and had to be fabricated here.
    let eligible: HashSet<u32> = nodes
        .iter()
        .filter(|n| {
            domains::is_domain_eligible(
                &domains::NodeFacts {
                    id: n.id,
                    language: n.language.as_str(),
                    kind: n.kind.as_str(),
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
    let symbol_name: HashMap<u32, &str> = nodes.iter().map(|n| (n.id, n.name.as_str())).collect();

    // A single-dominant repo keeps within-anchor communities too, matching the
    // producer: otherwise a one-package repo has no domains at all. Counted over
    // the SAME eligible set the producer uses (non-test, non-container) and with
    // the same STRICT majority — a half-and-half repo is not single-dominant.
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
    let membership_pairs: Vec<(u32, u32)> = membership
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
    .into_iter()
    .filter_map(|d| {
        // `select_domains` drops any community without a nameable hub, so the
        // first ranked member always exists — stay total rather than index.
        let &hub = d.ranked.first()?;
        Some(DomainSelection {
            hub,
            ranked: d.ranked,
            packages: d
                .packages
                .iter()
                .map(|&(p, m, l)| (p.to_string(), m, l))
                .collect(),
        })
    })
    .collect()
}

/// List the workspace's cross-package domains, or one domain with its spanned
/// packages and central members.
pub async fn list_domains(
    state: &ServerState,
    args: &ListDomainsArgs,
) -> Result<ListResponse<DomainView>, McpError> {
    let want = args.domain.clone();
    let args_pagination = args.pagination.clone();
    state
        .with_db(|h| async move {
            let nodes = h.read.scan_aggregate_nodes().await.map_err(internal)?;
            let edges = h.read.scan_aggregate_edges().await.map_err(internal)?;
            let flat = h
                .read
                .scan_analysis_flat_communities()
                .await
                .map_err(internal)?;
            let membership = h
                .read
                .scan_analysis_node_membership()
                .await
                .map_err(internal)?;

            let selected = domain_selections(&nodes, &edges, &flat, &membership);
            let mut items: Vec<DomainView> = Vec::with_capacity(selected.len());
            for DomainSelection {
                hub,
                ranked,
                packages,
            } in selected
            {
                // The hub's pub_id is the domain's handle; a domain whose hub has
                // no resolvable symbol is not addressable, so skip it.
                let Some(hub_sym) = h
                    .read
                    .fetch_symbol_by_short_id(hub)
                    .await
                    .map_err(internal)?
                else {
                    continue;
                };
                let links = packages.iter().map(|&(_, _, l)| l).sum::<u64>() / 2;
                items.push(DomainView {
                    symbol: hub_sym.pub_id,
                    title: hub_sym.name,
                    size: ranked.len() as u64,
                    packages_count: packages.len() as u64,
                    links,
                    packages: Vec::new(),
                    central: Vec::new(),
                });
                // Detail only for a named domain; a bare listing stays flat.
                if let Some(w) = want.as_deref() {
                    let row = items.last_mut().expect("just pushed");
                    if row.symbol != w && row.title != w {
                        items.pop();
                        continue;
                    }
                    row.packages = packages
                        .iter()
                        .map(|(p, m, l)| SpannedPackageView {
                            package: p.clone(),
                            members: *m,
                            links: *l,
                        })
                        .collect();
                    for &id in ranked.iter().take(CENTRAL_CAP) {
                        if let Some(s) = h
                            .read
                            .fetch_symbol_by_short_id(id)
                            .await
                            .map_err(internal)?
                        {
                            row.central.push(DomainMemberView {
                                id: s.pub_id,
                                name: s.name,
                                kind: s.kind,
                            });
                        }
                    }
                }
            }

            // Heaviest first, ties by title — the same order the atlas renders.
            items.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.title.cmp(&b.title)));
            let (items, next) =
                super::support::page_axis_items(items, args_pagination.as_ref(), h.snapshot_id)?;
            Ok(ListResponse { items, next })
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenn_model::EdgeKind;
    use kenn_store::{
        AggregateEdgeRow, AggregateNodeRow, AnalysisFlatCommunityRow, AnalysisNodeMembershipRow,
    };

    fn node(id: u32, name: &str, kind: &str, lang: &str, anchor: &str) -> AggregateNodeRow {
        AggregateNodeRow {
            id,
            kind: kind.into(),
            name: name.into(),
            language: lang.into(),
            external: false,
            test: false,
            example: false,
            anchor_id: 0,
            anchor_name: anchor.into(),
        }
    }

    /// Same, but defined under an example/sample/demo path — the flag the
    /// aggregation pass persists so this query never has to see a path.
    fn example_node(id: u32, name: &str, anchor: &str) -> AggregateNodeRow {
        AggregateNodeRow {
            example: true,
            ..node(id, name, "struct", "rust", anchor)
        }
    }

    fn edge(src: u32, dst: u32) -> AggregateEdgeRow {
        AggregateEdgeRow {
            src_id: src,
            dst_id: dst,
            kind: EdgeKind::Calls,
            weight: 5,
        }
    }

    fn community(id: u32, size: u32, cross: bool) -> AnalysisFlatCommunityRow {
        AnalysisFlatCommunityRow {
            community_id: id,
            size,
            total_weight: 10,
            cross_anchor: cross,
            primary_anchor_id: 0,
            primary_anchor_name: "a".into(),
        }
    }

    fn member(short_id: u32, community: u32) -> AnalysisNodeMembershipRow {
        AnalysisNodeMembershipRow {
            short_id,
            flat_community_id: community,
            anchored_leaf_community_id: community,
        }
    }

    /// The query's eligibility projection must apply EVERY predicate the
    /// producer does. Each of these was a real divergence found by hand, one per
    /// test repo, before the predicate was shared:
    ///
    /// - a markdown node became a domain hub because the code-language filter
    ///   was missing (a docs-heavy repo reported a note as a domain);
    /// - a container (`module`/`namespace`/`package`) could seed a domain;
    /// - test and external nodes are never first-party architecture.
    ///
    /// Every excluded node sits in a SPANNED package on purpose — put them in an
    /// unrelated package and the span restriction masks the check, so the test
    /// would guard nothing.
    ///
    /// Mutation-checked per predicate: removing `is_code_lang`, `!n.test`, or the
    /// container check from `is_domain_eligible` each fails this. Removing
    /// `!n.external` does NOT — for this caller `node_anchor` already drops
    /// external nodes, so that predicate is belt-and-braces here and load-bearing
    /// only for a caller whose anchor map is less strict.
    #[test]
    fn ineligible_nodes_never_seed_a_domain() {
        // Four eligible code types across two packages, plus one of each kind
        // of node that must be excluded.
        let mut nodes = vec![
            node(1, "Alpha", "class", "rust", "core"),
            node(2, "Beta", "class", "rust", "core"),
            node(3, "Gamma", "class", "rust", "web"),
            node(4, "Delta", "class", "rust", "web"),
            node(10, "SomeNote", "document", "markdown", "core"),
            node(11, "some_module", "module", "rust", "core"),
        ];
        nodes.push({
            let mut n = node(12, "TestThing", "class", "rust", "core");
            n.test = true;
            n
        });
        nodes.push({
            let mut n = node(13, "VendorThing", "class", "rust", "core");
            n.external = true;
            n
        });

        // One community holding all of them, cross-anchor.
        let flat = vec![community(1, 8, true)];
        let membership: Vec<_> = [1, 2, 3, 4, 10, 11, 12, 13]
            .into_iter()
            .map(|id| member(id, 1))
            .collect();
        // Cross-package references, two distinct pairs so the link floor is met.
        let edges = vec![edge(1, 3), edge(2, 4)];

        let got = domain_selections(&nodes, &edges, &flat, &membership);
        assert_eq!(got.len(), 1, "one earned domain");
        let d = &got[0];
        for excluded in [10u32, 11, 12, 13] {
            assert!(
                !d.ranked.contains(&excluded),
                "node {excluded} (markdown / container / test / external) must not be a domain member"
            );
        }
        assert_eq!(d.ranked.len(), 4, "only the four eligible code types");
        let pkgs: Vec<&str> = d.packages.iter().map(|(p, _, _)| p.as_str()).collect();
        assert_eq!(
            pkgs,
            vec!["core", "web"],
            "the span is the two code packages"
        );
    }

    /// A span carried entirely by example code is not a span. This is the
    /// divergence that motivated persisting `example` on the node: while it was
    /// a producer-side path join, this query passed `example: false` and
    /// reported a domain whose whole second package was throwaway spikes — on
    /// kenn's own repo, where four `crates/kenn-store/examples/*.rs` functions
    /// earned a `kenn-embed`↔`kenn-store` "domain" the atlas never rendered.
    ///
    /// `other` exists to hold the single-dominant escape open: without a second
    /// populated package, `core` alone would be a strict majority of the
    /// eligible nodes and within-anchor communities would qualify anyway,
    /// masking the span check.
    ///
    /// Mutation-checked: passing `example: false` into `is_domain_eligible`
    /// here (the pre-change behaviour) makes this a domain spanning two
    /// packages.
    #[test]
    fn a_span_carried_only_by_example_code_is_not_a_domain() {
        let mut nodes = vec![
            node(1, "Alpha", "class", "rust", "core"),
            node(2, "Beta", "class", "rust", "core"),
            node(5, "Gamma", "class", "rust", "core"),
            node(6, "Delta", "class", "rust", "core"),
            // The second package's entire presence in this community is
            // example code — a spike that references the library.
            example_node(3, "SpikeOne", "web"),
            example_node(4, "SpikeTwo", "web"),
        ];
        // A second real package, so `core` is not a strict majority.
        nodes.extend((20..24).map(|id| node(id, "Other", "class", "rust", "other")));

        let flat = vec![community(1, 6, true)];
        let membership: Vec<_> = [1, 2, 5, 6, 3, 4]
            .into_iter()
            .map(|id| member(id, 1))
            .collect();
        // Two distinct cross-package references, so only the example flag can
        // be what withholds the span.
        let edges = vec![edge(1, 3), edge(2, 4)];

        let got = domain_selections(&nodes, &edges, &flat, &membership);
        assert!(
            got.is_empty(),
            "the community collapses to one package once example code is excluded, \
             so it is the package concept's job — got {:?}",
            got.iter()
                .map(|d| d
                    .packages
                    .iter()
                    .map(|(p, _, _)| p.as_str())
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>()
        );
    }

    /// The link floor counts distinct REFERENCES, not aggregate edge rows. The
    /// graph splits one reference across a row per kind, so counting rows made
    /// `MIN_DOMAIN_LINKS` a no-op: one reference cleared a floor of two.
    ///
    /// Mutation-checked: removing the `counted.insert` dedupe in
    /// `community_pair_links` makes this a domain.
    #[test]
    fn one_reference_split_across_kinds_is_not_a_span() {
        let nodes = vec![
            node(1, "Alpha", "class", "rust", "core"),
            node(2, "Beta", "class", "rust", "core"),
            node(3, "Gamma", "class", "rust", "web"),
            node(4, "Delta", "class", "rust", "web"),
        ];
        let flat = vec![community(1, 4, true)];
        let membership: Vec<_> = (1..=4).map(|id| member(id, 1)).collect();
        // ONE cross-package reference, emitted as three kinds — three rows.
        let edges = vec![
            AggregateEdgeRow {
                src_id: 1,
                dst_id: 3,
                kind: EdgeKind::Calls,
                weight: 3,
            },
            AggregateEdgeRow {
                src_id: 1,
                dst_id: 3,
                kind: EdgeKind::TypeUse,
                weight: 2,
            },
            AggregateEdgeRow {
                src_id: 1,
                dst_id: 3,
                kind: EdgeKind::FieldAccess,
                weight: 2,
            },
        ];
        assert!(
            domain_selections(&nodes, &edges, &flat, &membership).is_empty(),
            "one reference is a mention, however many edge kinds it emits"
        );
    }
}
