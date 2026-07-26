//! `list_contracts` — the contract axis of the graph, as a query.
//!
//! First-party interfaces / base types whose implementers span more than one
//! package, read STRAIGHT from the `implements` / `extends_type` edges of the
//! published snapshot. Both this and the atlas go through
//! `kenn_indexer::atlas::contracts`, so the floor can never mean one thing in
//! the document and another at the prompt.
//!
//! The render caps are deliberately NOT applied here: a query must be able to
//! reach every contract, and every implementer of one. Counts are the honest
//! totals, not a truncated view that reads as complete.

use std::collections::HashMap;

use kenn_indexer::atlas::contracts;
use kenn_model::EdgeKind;
use kenn_store::api::Reader;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::McpError;
use crate::types::ListResponse;

use super::{internal, ServerState};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ListContractsArgs {
    /// Restrict to one contract — its `pub_id`, or its title. A title is a
    /// QUERY, not an identifier: two packages can each define an `IValidator`,
    /// so when a title matches more than one contract EVERY match is returned,
    /// each tagged with its own `pub_id`. Never an error, never a second call.
    #[serde(default)]
    pub contract: Option<String>,
    /// Rows per response and the continuation cursor. The axis is computed
    /// whole and ordered deterministically, so this is a plain offset walk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<crate::types::Pagination>,
}

/// One implementer of a contract — a resolvable handle to drill in.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ImplementerView {
    /// The stable `pub_id` — feed it straight to `kenn get` / `kenn list`.
    pub id: String,
    pub name: String,
    pub package: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ContractView {
    /// The contract type's own `pub_id` — its resolvable handle, and what you
    /// name to get its implementers. ALWAYS serialized so every row carries the
    /// same fields and the listing stays a flat table. First column: it is the
    /// id a reader acts on.
    pub symbol: String,
    pub title: String,
    /// `interface`, `class` (a base type), `protocol`, …
    pub kind: String,
    /// The package the contract is defined in.
    pub defined_in: String,
    /// Distinct implementers, and the packages they span. These are the counts
    /// the selection was derived from, and they are NOT capped.
    pub implementers_count: u64,
    pub package_span: u64,
    /// Populated only when a single `contract` was requested — the full listing
    /// would be quadratic and unreadable.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub implementers: Vec<ImplementerView>,
}

/// One selected contract, resolved down to ids the async shell can look up.
#[derive(Debug, Clone)]
pub struct ContractSelection {
    pub node: u32,
    pub name: String,
    pub kind: String,
    pub defined_in: String,
    /// `(package, implementer node ids)`, widest package first, uncapped.
    pub by_package: Vec<(String, Vec<u32>)>,
    pub total_implementers: u64,
    pub package_span: u64,
}

/// The pure selection: project snapshot rows into the shared rule.
///
/// Separated from the async shell so the rules are testable without a published
/// snapshot behind them.
#[must_use]
pub fn contract_selections(
    nodes: &[kenn_store::AggregateNodeRow],
    edges: &[kenn_store::AggregateEdgeRow],
) -> Vec<ContractSelection> {
    // First-party + anchored only: an external node is a vendored dependency,
    // and `<unanchored>` is the no-package sentinel.
    let node_anchor: HashMap<u32, &str> = nodes
        .iter()
        .filter(|n| !n.external && n.anchor_name != "<unanchored>")
        .map(|n| (n.id, n.anchor_name.as_str()))
        .collect();
    let node_info: HashMap<u32, contracts::NodeInfo<'_>> = nodes
        .iter()
        .map(|n| {
            (
                n.id,
                contracts::NodeInfo {
                    name: n.name.as_str(),
                    kind: n.kind.as_str(),
                    test: n.test,
                },
            )
        })
        .collect();
    let symbol_name: HashMap<u32, &str> = nodes.iter().map(|n| (n.id, n.name.as_str())).collect();

    // Direction is load-bearing: the edge points implementer → contract, and
    // the aggregate graph preserves it (endpoints are not sorted).
    let is_a_edges: Vec<(u32, u32)> = edges
        .iter()
        .filter(|e| matches!(e.kind, EdgeKind::Implements | EdgeKind::ExtendsType))
        .map(|e| (e.src_id, e.dst_id))
        .collect();

    contracts::select_contracts(&is_a_edges, &node_anchor, &node_info, &symbol_name)
        .into_iter()
        .map(|c| ContractSelection {
            node: c.node,
            name: c.name.to_string(),
            kind: c.kind.to_string(),
            defined_in: c.defined_in.to_string(),
            by_package: c
                .by_package
                .into_iter()
                .map(|(p, ids)| (p.to_string(), ids))
                .collect(),
            total_implementers: c.total_implementers,
            package_span: c.package_span,
        })
        .collect()
}

/// List the workspace's cross-package contracts, or one contract with its
/// implementers grouped by package.
///
/// An empty result is a real answer, not a failure: Rust and Go keep
/// abstractions package-local, so their contracts axis is legitimately empty.
pub async fn list_contracts(
    state: &ServerState,
    args: &ListContractsArgs,
) -> Result<ListResponse<ContractView>, McpError> {
    let want = args.contract.clone();
    let args_pagination = args.pagination.clone();
    state
        .with_db(|h| async move {
            let nodes = h.read.scan_aggregate_nodes().await.map_err(internal)?;
            let edges = h.read.scan_aggregate_edges().await.map_err(internal)?;

            let mut items: Vec<ContractView> = Vec::new();
            for c in contract_selections(&nodes, &edges) {
                // The contract's own pub_id is its handle; one that resolves to
                // no symbol is not addressable, so skip it.
                let Some(sym) = h
                    .read
                    .fetch_symbol_by_short_id(c.node)
                    .await
                    .map_err(internal)?
                else {
                    continue;
                };
                // A name argument is a QUERY: it matches the pub_id OR the
                // title, and a title that matches several keeps them all.
                if let Some(w) = want.as_deref() {
                    if sym.pub_id != w && c.name != w {
                        continue;
                    }
                }
                let mut implementers = Vec::new();
                if want.is_some() {
                    for (pkg, ids) in &c.by_package {
                        for &id in ids {
                            if let Some(s) = h
                                .read
                                .fetch_symbol_by_short_id(id)
                                .await
                                .map_err(internal)?
                            {
                                implementers.push(ImplementerView {
                                    id: s.pub_id,
                                    name: s.name,
                                    package: pkg.clone(),
                                });
                            }
                        }
                    }
                }
                items.push(ContractView {
                    symbol: sym.pub_id,
                    title: c.name,
                    kind: c.kind,
                    defined_in: c.defined_in,
                    implementers_count: c.total_implementers,
                    package_span: c.package_span,
                    implementers,
                });
            }

            let (items, next) =
                super::support::page_axis_items(items, args_pagination.as_ref(), h.snapshot_id)?;
            Ok(ListResponse { items, next })
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenn_store::{AggregateEdgeRow, AggregateNodeRow};

    fn node(id: u32, name: &str, anchor: &str, external: bool, test: bool) -> AggregateNodeRow {
        AggregateNodeRow {
            id,
            kind: "interface".into(),
            name: name.into(),
            language: "csharp".into(),
            external,
            test,
            example: false,
            anchor_id: 0,
            anchor_name: anchor.into(),
        }
    }

    fn is_a(src: u32, dst: u32) -> AggregateEdgeRow {
        AggregateEdgeRow {
            src_id: src,
            dst_id: dst,
            kind: EdgeKind::Implements,
            weight: 2,
        }
    }

    /// A contract earns the axis only by spanning MORE THAN ONE package — a
    /// single-package interface plus its impls is local detail the package
    /// concept already covers. Mutation-checked: dropping the `MIN_CONTRACT_PKGS`
    /// floor admits `Local`, which has two implementers but both in one package.
    #[test]
    fn single_package_interfaces_are_excluded() {
        let nodes = vec![
            node(1, "Shared", "core", false, false),
            node(2, "ImplA", "web", false, false),
            node(3, "ImplB", "cli", false, false),
            node(10, "Local", "core", false, false),
            node(11, "LocalImplA", "core", false, false),
            node(12, "LocalImplB", "core", false, false),
        ];
        let edges = vec![is_a(2, 1), is_a(3, 1), is_a(11, 10), is_a(12, 10)];
        let got = contract_selections(&nodes, &edges);
        let titles: Vec<&str> = got.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            titles,
            vec!["Shared"],
            "only the cross-package contract qualifies; `Local` spans one package"
        );
        assert_eq!(got[0].package_span, 2);
        assert_eq!(got[0].total_implementers, 2);
    }

    /// Test doubles are not architecture, and a vendored type is not ours.
    /// Mutation-checked: dropping either filter re-admits the excluded package
    /// and pushes `package_span` to 3.
    #[test]
    fn test_and_external_implementers_are_excluded() {
        let nodes = vec![
            node(1, "Shared", "core", false, false),
            node(2, "ImplA", "web", false, false),
            node(3, "ImplB", "cli", false, false),
            node(4, "FakeImpl", "tests", false, true),
            node(5, "VendorImpl", "vendor", true, false),
        ];
        let edges = vec![is_a(2, 1), is_a(3, 1), is_a(4, 1), is_a(5, 1)];
        let got = contract_selections(&nodes, &edges);
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].package_span, 2,
            "the test double and the vendored implementer are both excluded"
        );
        let pkgs: Vec<&str> = got[0].by_package.iter().map(|(p, _)| p.as_str()).collect();
        assert!(!pkgs.contains(&"tests") && !pkgs.contains(&"vendor"));
    }

    /// A title is NOT unique — two packages can each define `IValidator`, which
    /// is why the atlas has to disambiguate colliding concept slugs. Both must
    /// survive selection as distinct contracts, each addressable by its own id;
    /// collapsing them to one is the bug this guards.
    #[test]
    fn same_named_contracts_in_different_packages_stay_distinct() {
        let nodes = vec![
            node(1, "IValidator", "billing", false, false),
            node(2, "IValidator", "identity", false, false),
            node(3, "A1", "web", false, false),
            node(4, "A2", "cli", false, false),
            node(5, "B1", "web", false, false),
            node(6, "B2", "cli", false, false),
        ];
        let edges = vec![is_a(3, 1), is_a(4, 1), is_a(5, 2), is_a(6, 2)];
        let got = contract_selections(&nodes, &edges);
        assert_eq!(got.len(), 2, "one title, two contracts — both kept");
        assert!(got.iter().all(|c| c.name == "IValidator"));
        let mut defined: Vec<&str> = got.iter().map(|c| c.defined_in.as_str()).collect();
        defined.sort_unstable();
        assert_eq!(
            defined,
            vec!["billing", "identity"],
            "each is addressable by the package it is defined in, hence by its own id"
        );
    }

    /// An empty axis is a real answer: Rust and Go keep abstractions
    /// package-local, so they legitimately have no cross-package contracts.
    #[test]
    fn no_cross_package_contracts_is_an_empty_selection() {
        let nodes = vec![node(1, "Solo", "core", false, false)];
        assert!(contract_selections(&nodes, &[]).is_empty());
    }
}
