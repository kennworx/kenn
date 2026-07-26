//! Contract-axis SELECTION: which first-party interfaces / base types have
//! implementers spanning more than one package, and how those implementers group.
//!
//! Read STRAIGHT from the `implements` / `extends_type` edges — explicit,
//! complete, and deterministic, unlike a clustering that merges an interface with
//! a fragile subset of its implementers. Extracted so the atlas producer and the
//! contracts query select the SAME contracts from their different inputs; see
//! [`super::coupling`] for the pattern and the reason.
//!
//! Selection only. The render caps (`MAX_CONTRACTS`, `MAX_CONTRACT_PKGS`,
//! `MAX_IMPLEMENTERS_PER_PKG`) and the concept-id slugs stay in the producer: a
//! cap is presentation policy, and a query must be able to reach every contract.

use std::collections::HashMap;

use kenn_model::ShortId;

/// Packages a contract's implementers must span before it earns a concept — a
/// single-package interface+impls is local detail the package concept covers.
pub const MIN_CONTRACT_PKGS: usize = 2;

/// The aggregate-node facts selection needs, projected by the caller.
#[derive(Debug, Clone, Copy)]
pub struct NodeInfo<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub test: bool,
}

/// One contract whose implementers span more than one package.
#[derive(Debug, Clone)]
pub struct SelectedContract<'a> {
    /// The contract type's aggregate node.
    pub node: ShortId,
    pub name: &'a str,
    pub kind: &'a str,
    /// The package the contract is defined in.
    pub defined_in: &'a str,
    /// Implementers grouped by package, widest first and UNCAPPED. Within a
    /// package they are sorted by symbol name (ties by id) for a readable table.
    pub by_package: Vec<(&'a str, Vec<ShortId>)>,
    /// Distinct implementers across every package, before any render cap.
    pub total_implementers: u64,
    /// Distinct implementer packages, before any render cap.
    pub package_span: u64,
}

/// Select every cross-package contract, widest span first.
///
/// `is_a_edges` are `(implementer, contract)` pairs the caller has already
/// filtered to the is-a kinds (`implements` / `extends_type`). Both endpoints
/// must be first-party (present in `node_anchor`), named, and non-test — a
/// production contract's test doubles are not its architecture, matching
/// domain/central eligibility.
#[must_use]
#[expect(
    clippy::implicit_hasher,
    reason = "both callers pass the std-default HashMap; generalizing over BuildHasher only adds noise"
)]
pub fn select_contracts<'a>(
    is_a_edges: &[(ShortId, ShortId)],
    node_anchor: &HashMap<ShortId, &'a str>,
    node_info: &HashMap<ShortId, NodeInfo<'a>>,
    symbol_name: &HashMap<ShortId, &str>,
) -> Vec<SelectedContract<'a>> {
    // contract → implementer package → implementer node ids.
    let mut by_contract: HashMap<ShortId, HashMap<&str, Vec<ShortId>>> = HashMap::new();
    for &(src, dst) in is_a_edges {
        let (Some(&ipkg), Some(_)) = (node_anchor.get(&src), node_anchor.get(&dst)) else {
            continue;
        };
        let (Some(contract), Some(implementer)) = (node_info.get(&dst), node_info.get(&src)) else {
            continue;
        };
        if contract.test
            || implementer.test
            || contract.name.is_empty()
            || implementer.name.is_empty()
        {
            continue;
        }
        by_contract
            .entry(dst)
            .or_default()
            .entry(ipkg)
            .or_default()
            .push(src);
    }

    let name_of = |s: ShortId| symbol_name.get(&s).copied().unwrap_or("");
    let mut out: Vec<SelectedContract<'a>> = Vec::new();
    for (&cid, pkgs) in &by_contract {
        if pkgs.len() < MIN_CONTRACT_PKGS {
            continue;
        }
        // Both are guaranteed present — `cid` only entered `by_contract` when its
        // node and anchor resolved above — but stay total rather than index.
        let (Some(contract), Some(&def_pkg)) = (node_info.get(&cid), node_anchor.get(&cid)) else {
            continue;
        };
        let mut by_package: Vec<(&'a str, Vec<ShortId>)> = pkgs
            .iter()
            .map(|(&pkg, ids)| {
                let mut ids: Vec<ShortId> = ids.clone();
                ids.sort_unstable();
                ids.dedup();
                // Sort by name for a readable table, ties by id for determinism.
                ids.sort_by(|&a, &b| name_of(a).cmp(name_of(b)).then(a.cmp(&b)));
                (pkg, ids)
            })
            .collect();
        let total_implementers: u64 = by_package.iter().map(|(_, ids)| ids.len() as u64).sum();
        let package_span = by_package.len() as u64;
        // Widest first: the package a reader looks at, then determinism by name.
        by_package.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));
        out.push(SelectedContract {
            node: cid,
            name: contract.name,
            kind: contract.kind,
            defined_in: def_pkg,
            by_package,
            total_implementers,
            package_span,
        });
    }

    // The broadest extension points lead; ties by implementer count, then name,
    // then node id. The id tie-break is load-bearing: `by_contract` is a HashMap,
    // so two same-named contracts with equal span and count would otherwise keep
    // their random iteration order, and `dedupe_contract_ids` would hand the
    // `-2` suffix to a different one on each re-index.
    out.sort_by(|a, b| {
        b.package_span
            .cmp(&a.package_span)
            .then_with(|| b.total_implementers.cmp(&a.total_implementers))
            .then_with(|| a.name.cmp(b.name))
            .then_with(|| a.node.cmp(&b.node))
    });
    out
}
