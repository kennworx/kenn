//! Lifecycle helpers — the `CodeNodeResolver` trait, the staleness check
//! that drives read-time freshness flags, and the supersede / tombstone
//! tag conventions that `search_findings` filters by.

use std::collections::HashSet;

use crate::api::types::Finding;

/// Prefix that distinguishes a finding id from a code-graph node id in
/// the unified `parent_ids` space.
pub(super) const FINDING_ID_PREFIX: &str = "fnd_";

/// Reserved tag marking a finding as a *directive* — a do/don't rule. A
/// directive also carries a `polarity:*` tag (read by the agent-side guardrail,
/// not the store) and is the subject of the before-commit violation check.
pub(super) const TAG_DIRECTIVE: &str = "directive";

/// Reserved tag marking a finding as a *guide* — orientation / how-to context,
/// retrieved alongside directives but never violation-checked.
pub(super) const TAG_GUIDE: &str = "guide";

/// True iff `finding` carries the `directive` or `guide` tag — the retrievable
/// steering set that `find_directives` filters to (no new record kind).
pub(super) fn is_directive_or_guide(finding: &Finding) -> bool {
    finding
        .tags
        .iter()
        .any(|t| t == TAG_DIRECTIVE || t == TAG_GUIDE)
}

/// Resolves whether a code-graph node id still exists in the current
/// branch's code graph — the membership test that drives read-time
/// staleness. A code-node id has the form `<lang>:<pub_id>`.
pub trait CodeNodeResolver {
    /// True iff `code_node_id` resolves in the current code graph.
    fn contains(&self, code_node_id: &str) -> bool;
}

/// True iff any of `finding`'s code-graph `parent_ids` (the non-`fnd_`
/// entries) no longer resolves under `resolver`. Finding-id parents are
/// not code evidence and are ignored.
#[must_use]
pub fn finding_is_stale(finding: &Finding, resolver: &impl CodeNodeResolver) -> bool {
    finding
        .parent_ids
        .iter()
        .filter(|p| !p.starts_with(FINDING_ID_PREFIX))
        .any(|code_id| !resolver.contains(code_id))
}

/// True iff `finding` carries a tag with the given lifecycle `prefix`
/// (`"supersedes:"` or `"tombstone:"`).
pub(super) fn carries_lifecycle_tag(finding: &Finding, prefix: &str) -> bool {
    finding.tags.iter().any(|t| t.starts_with(prefix))
}

/// Build the `(superseded, tombstoned)` id sets from every finding's
/// tags: every id named by a `supersedes:` tag, and every id named by a
/// `tombstone:` tag.
pub(super) fn lifecycle_sets(findings: &[Finding]) -> (HashSet<String>, HashSet<String>) {
    let mut superseded = HashSet::new();
    let mut tombstoned = HashSet::new();
    for f in findings {
        for tag in &f.tags {
            if let Some(target) = tag.strip_prefix("supersedes:") {
                superseded.insert(target.to_owned());
            } else if let Some(target) = tag.strip_prefix("tombstone:") {
                tombstoned.insert(target.to_owned());
            }
        }
    }
    (superseded, tombstoned)
}
