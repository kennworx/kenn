//! Directive/guide retrieval — the ranking behind `find_directives`.
//!
//! `find_directives(paths)` fuses two ranked legs over the directive/guide
//! findings: a **structural** leg (the finding's anchors match a changed path —
//! exact file or an ancestor directory) ranked by recency-weighted liveness, and
//! a **semantic** leg (the body vector's cosine to a supplied query vector).
//! The two ranked id lists are blended with reciprocal-rank fusion. Superseded
//! and tombstoned findings are excluded; each hit carries the read-time `stale`
//! flag. When no query vector is supplied (or the embedder is cold) the semantic
//! leg is empty and ranking degrades to the structural leg alone — anchors are
//! committed and resolvable with no index.
//!
//! The pure ranking lives here (no I/O) so it is unit-testable; the store method
//! gathers the inputs (candidates, folded anchors, per-candidate cosine).

use std::collections::HashMap;

use crate::api::types::{Finding, FindingHit};

use super::anchor::Anchor;
use super::lifecycle::{finding_is_stale, CodeNodeResolver};
use crate::clock::Timestamp;

/// Half-life (seconds) of the anchor-liveness recency decay (~30 days): an
/// anchor re-attached recently weighs ~1, one untouched for a half-life ~0.5.
const LIVENESS_HALF_LIFE_SECS: f64 = 30.0 * 24.0 * 3600.0;
/// Reciprocal-rank-fusion damping (matches the code-search fusion's `RRF_K`).
const RRF_K: f64 = 60.0;

/// A finding whose anchors no longer resolve on disk — reported by
/// `check_anchors` so the agent can repair moves/deletions before a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokenAnchors {
    /// The finding whose anchors are broken.
    pub finding_id: String,
    /// The anchor paths (file or directory) that no longer resolve.
    pub anchors: Vec<String>,
}

/// A finding whose anchored **files** still exist but changed content since the
/// sha recorded at attach — reported by `check_anchors` so the agent can re-read
/// a directive whose ground truth moved before relying on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftedAnchors {
    /// The finding whose anchored files drifted.
    pub finding_id: String,
    /// The anchor file paths whose content changed since attach.
    pub anchors: Vec<String>,
}

/// A **claim** — a finding asserting something about the current state of the
/// code — whose anchored content has changed since the claim was recorded, so
/// the assertion may no longer be true.
///
/// Reported separately from [`DriftedAnchors`] because the two ask different
/// questions. Drift asks whether bytes moved, which for a rule is incidental.
/// This asks whether an assertion still holds, which is the only question a
/// claim raises — and merging them is what leaves the signal unread: on this
/// repository the drift list ran to 127 entries, where a claim that had stopped
/// being true was indistinguishable from a rule whose file was merely touched.
///
/// This says nothing about whether the claim still holds. The store cannot know
/// whether the change fixed it, worsened it, or missed it — asserting any of
/// those would replace one stale fact with another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiedClaim {
    /// The claim whose anchored content changed.
    pub finding_id: String,
    /// The anchor file paths whose content changed since the claim was recorded.
    pub anchors: Vec<String>,
}

/// The read-time anchor-health buckets `check_anchors` reports: paths that no
/// longer resolve (`broken`), rule files whose content changed (`drifted`), and
/// claims whose content changed and therefore need re-verifying (`unverified`).
///
/// Superseded and tombstoned findings appear in none of them. They are already
/// excluded from retrieval, so they can never surface as guidance and repairing
/// their anchors is busywork — measured here, 26 of 127 drifted entries were
/// superseded ancestors, a fifth of the list that trains a reader to skim it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnchorHealth {
    pub broken: Vec<BrokenAnchors>,
    pub drifted: Vec<DriftedAnchors>,
    pub unverified: Vec<UnverifiedClaim>,
}

/// One candidate directive/guide finding plus the inputs the ranking needs.
pub(super) struct DirectiveInput {
    pub finding: Finding,
    /// The finding's current anchor set (folded from its log).
    pub anchors: Vec<Anchor>,
    /// Cosine of the query vector against this finding's body vector — `None`
    /// when no query vector was supplied or the finding has no sidecar vector.
    pub semantic: Option<f32>,
}

/// True iff `anchor` (a file or directory path) applies to changed `path`:
/// an exact match, or `anchor` is an ancestor directory of `path`.
fn anchor_applies(anchor: &str, path: &str) -> bool {
    if anchor == path {
        return true;
    }
    let dir = anchor.strip_suffix('/').unwrap_or(anchor);
    path.starts_with(&format!("{dir}/"))
}

/// Recency-weighted liveness of one anchor at `now`: its attach count decayed
/// by how long since it was last attached.
#[expect(
    clippy::cast_precision_loss,
    reason = "Unix-second timestamps and small attach counts are far under 2^52"
)]
fn anchor_liveness(anchor: &Anchor, now: Timestamp) -> f64 {
    let age = (now.unix() - anchor.recency.unix()).max(0) as f64;
    let decay = 0.5_f64.powf(age / LIVENESS_HALF_LIFE_SECS);
    f64::from(anchor.attach_count) * decay
}

/// The finding's structural liveness for `paths`: the max liveness over its
/// anchors that apply to any changed path, or `None` if none apply.
fn structural_liveness(input: &DirectiveInput, paths: &[String], now: Timestamp) -> Option<f64> {
    input
        .anchors
        .iter()
        .filter(|a| paths.iter().any(|p| anchor_applies(&a.path, p)))
        .map(|a| anchor_liveness(a, now))
        .fold(None, |acc, l| Some(acc.map_or(l, |m: f64| m.max(l))))
}

/// Add one best-first ranked arm into the RRF score map (weight `w`).
fn rrf_into(scores: &mut HashMap<String, f64>, ranked_ids: &[String], w: f64) {
    for (i, id) in ranked_ids.iter().enumerate() {
        #[expect(
            clippy::cast_precision_loss,
            reason = "arm length is a small candidate count, far under 2^52"
        )]
        let rank = (i + 1) as f64;
        *scores.entry(id.clone()).or_default() += w / (RRF_K + rank);
    }
}

/// Rank directive/guide candidates for `paths`, fusing the structural and
/// semantic legs and attaching the read-time `stale` flag. Inputs are assumed
/// pre-filtered to directives/guides with superseded/tombstoned already removed.
pub(super) fn rank_directives(
    inputs: &[DirectiveInput],
    paths: &[String],
    now: Timestamp,
    limit: usize,
    resolver: &impl CodeNodeResolver,
) -> Vec<FindingHit> {
    // Structural arm: candidates whose anchors apply to a changed path, ordered
    // by descending liveness.
    let mut structural: Vec<(String, f64)> = inputs
        .iter()
        .filter_map(|i| structural_liveness(i, paths, now).map(|l| (i.finding.id.clone(), l)))
        .collect();
    structural.sort_by(|a, b| b.1.total_cmp(&a.1));
    let structural_ids: Vec<String> = structural.into_iter().map(|(id, _)| id).collect();

    // Semantic arm: candidates with a cosine score, ordered by descending cosine.
    let mut semantic: Vec<(String, f32)> = inputs
        .iter()
        .filter_map(|i| i.semantic.map(|s| (i.finding.id.clone(), s)))
        .collect();
    semantic.sort_by(|a, b| b.1.total_cmp(&a.1));
    let semantic_ids: Vec<String> = semantic.into_iter().map(|(id, _)| id).collect();

    let mut scores: HashMap<String, f64> = HashMap::new();
    rrf_into(&mut scores, &structural_ids, 1.0);
    rrf_into(&mut scores, &semantic_ids, 1.0);

    let by_id: HashMap<&str, &Finding> = inputs
        .iter()
        .map(|i| (i.finding.id.as_str(), &i.finding))
        .collect();
    let mut hits: Vec<FindingHit> = scores
        .into_iter()
        .filter_map(|(id, score)| {
            let finding = (*by_id.get(id.as_str())?).clone();
            let stale = finding_is_stale(&finding, resolver);
            #[expect(
                clippy::cast_possible_truncation,
                reason = "RRF scores are small positive f64 sums; f32 is ample"
            )]
            let score = score as f32;
            Some(FindingHit {
                finding,
                score,
                stale,
                // Both need file I/O (workspace root + hashing), which this
                // pure ranking deliberately avoids; the store method fills
                // them in.
                drifted: false,
                unverified: false,
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.finding.id.cmp(&b.finding.id))
    });
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::{anchor_applies, rank_directives, DirectiveInput};
    use crate::api::types::Finding;
    use crate::clock::Timestamp;
    use crate::db::findings::anchor::Anchor;
    use crate::db::findings::lifecycle::CodeNodeResolver;

    struct AllPresent;
    impl CodeNodeResolver for AllPresent {
        fn contains(&self, _: &str) -> bool {
            true
        }
    }

    /// A `Timestamp` at `secs` Unix seconds (test fixture).
    fn ts(secs: i64) -> Timestamp {
        time::OffsetDateTime::from_unix_timestamp(secs)
            .unwrap()
            .into()
    }

    fn anchor(path: &str, recency_secs: i64, count: u32) -> Anchor {
        Anchor {
            path: path.to_owned(),
            recency: ts(recency_secs),
            attach_count: count,
            sha: None,
            verified_sha: None,
            verified: None,
            origin_sha: None,
        }
    }

    fn directive(id: &str, anchors: Vec<Anchor>, semantic: Option<f32>) -> DirectiveInput {
        DirectiveInput {
            finding: Finding {
                id: id.to_owned(),
                text: format!("rule {id}"),
                embedding: None,
                tags: vec!["directive".to_owned()],
                parent_ids: vec![],
                created_at: ts(0),
            },
            anchors,
            semantic,
        }
    }

    #[test]
    fn anchor_applies_exact_and_ancestor_dir() {
        assert!(anchor_applies("a/b.rs", "a/b.rs"));
        assert!(anchor_applies("a/", "a/b.rs")); // dir anchor covers subtree
        assert!(anchor_applies("a", "a/b.rs")); // dir without trailing slash
        assert!(!anchor_applies("a/b.rs", "a/c.rs"));
        assert!(!anchor_applies("ab/", "a/b.rs")); // not a prefix boundary
    }

    #[test]
    fn structural_match_by_file_and_ancestor_dir() {
        let inputs = vec![
            directive("fnd_file", vec![anchor("crates/x/server.rs", 100, 1)], None),
            directive("fnd_dir", vec![anchor("crates/x/", 100, 1)], None),
            directive("fnd_other", vec![anchor("crates/y/z.rs", 100, 1)], None),
        ];
        let paths = vec!["crates/x/server.rs".to_owned()];
        let hits = rank_directives(&inputs, &paths, ts(100), 10, &AllPresent);
        let ids: Vec<&str> = hits.iter().map(|h| h.finding.id.as_str()).collect();
        assert!(ids.contains(&"fnd_file"));
        assert!(ids.contains(&"fnd_dir"));
        assert!(!ids.contains(&"fnd_other"), "unrelated anchor excluded");
    }

    #[test]
    fn liveness_orders_structural_hits() {
        // Same anchored path, different liveness: recent+frequent ranks first.
        let inputs = vec![
            directive("fnd_cold", vec![anchor("f.rs", 0, 1)], None),
            directive("fnd_hot", vec![anchor("f.rs", 1_000_000, 5)], None),
        ];
        let paths = vec!["f.rs".to_owned()];
        let hits = rank_directives(&inputs, &paths, ts(1_000_000), 10, &AllPresent);
        assert_eq!(hits.first().unwrap().finding.id, "fnd_hot");
    }

    #[test]
    fn empty_when_no_path_or_semantic_match() {
        let inputs = vec![directive("fnd_a", vec![anchor("x.rs", 0, 1)], None)];
        let hits = rank_directives(&inputs, &["other.rs".to_owned()], ts(0), 10, &AllPresent);
        assert!(hits.is_empty());
    }

    #[test]
    fn semantic_only_match_surfaces_without_anchor() {
        // No anchor match, but a query-vector cosine is present → still returned.
        let inputs = vec![directive("fnd_sem", vec![anchor("x.rs", 0, 1)], Some(0.9))];
        let hits = rank_directives(
            &inputs,
            &["unrelated.rs".to_owned()],
            ts(0),
            10,
            &AllPresent,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].finding.id, "fnd_sem");
    }
}
