//! Anchor + liveness event logs — `.kenn/findings/<id>.anchor.jsonl`.
//!
//! A finding's *anchors* (the files/dirs it applies to) and their liveness are a
//! per-finding append-only event log, kept separate from the immutable
//! `<id>.md` record because file/dir paths get moved, renamed, and deleted.
//! Event kinds are `attach`, `rename`, `detach`, and `verify`; a repeat
//! `attach` to a path already in the set is the liveness signal (there is no
//! separate confirm event). The current anchor set and per-anchor liveness are
//! a fold over the log. One JSON object per line — appends from two branches
//! union with no conflict on `git merge`.
//!
//! **`attach` and `verify` are different facts and write different fields.**
//! `attach` says "this finding applied to my change" and the pre-commit ritual
//! writes it in bulk; `verify` says "I read this claim against the code at this
//! sha and here is what I found". Letting the first stand in for the second
//! would let one sweep declare a store's worth of claims re-read without anyone
//! having read one — so the fold keeps two shas, and no `attach` can touch the
//! verification one.
//!
//! The fold reports each current anchor's `recency` (latest `attach` ts) and
//! `attach_count`. The recency-*weighting* (decay by the current clock) is
//! applied by the retrieval layer (`find_directives`), not baked into the fold,
//! so the fold stays deterministic and clock-free.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::types::DbError;
use crate::clock::Timestamp;

/// One event in a finding's anchor log. `ts` is a [`Timestamp`] supplied by the
/// caller (never read from the clock here, so the log is reproducible and
/// testable), serialized as an RFC 3339 (ISO 8601) string — human-readable in
/// the committed `.anchor.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum AnchorEvent {
    /// The finding applies to `anchor` (a file or directory path). A repeat
    /// `Attach` to an existing path is the liveness signal.
    Attach {
        anchor: String,
        ts: Timestamp,
        /// xxh64 hex of the anchored **file** at attach time, when it is a
        /// readable file. `None` for directory anchors, unreadable paths, and
        /// logs written before shas were recorded — all treated as live (drift
        /// unknown). Omitted from the serialized line when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha: Option<String>,
    },
    /// An anchored path moved — carry its liveness from `from` to `to`.
    Rename {
        from: String,
        to: String,
        ts: Timestamp,
    },
    /// The finding no longer applies to `anchor`.
    Detach { anchor: String, ts: Timestamp },
    /// Someone read the claim against the code at `sha` and reports what they
    /// found.
    ///
    /// Deliberately NOT `Attach`. `Attach` means "this applied to my change"
    /// and is written in bulk by the pre-commit ritual; if it doubled as
    /// verification, one sweep would declare a store's worth of claims true
    /// without anyone having read one. Carrying its own sha is what makes that
    /// impossible rather than merely discouraged.
    Verify {
        anchor: String,
        ts: Timestamp,
        /// The content the claim was judged against.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha: Option<String>,
        outcome: Outcome,
    },
}

/// What reading a claim against the current code found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Still true as written.
    StillTrue,
    /// No longer true. Recorded for the trail; the claim is retired by
    /// superseding it with one describing the current state, not by deleting.
    NoLongerTrue,
    /// True in part. A distinct outcome because the motivating incident was a
    /// successor asserting a flat FIXED where the fix covered one case and left
    /// a residue — which then read as untouched outstanding work, and acting on
    /// that reading removed a load-bearing placeholder.
    PartlyTrue,
}

/// A current anchor with its folded liveness facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// File or directory path the finding applies to.
    pub path: String,
    /// The latest `attach` timestamp for this path.
    pub recency: Timestamp,
    /// How many `attach` events this path has accrued. Retrieval applies a
    /// recency weighting on top — this is not the rank itself.
    pub attach_count: u32,
    /// The most-recent `attach`'s content sha (xxh64 hex), carried across a
    /// `rename`. `None` when unknown (directory anchor, unreadable path, or a
    /// log predating shas) — drift detection treats `None` as live.
    pub sha: Option<String>,
    /// The content sha the claim was last *verified* against, from a `Verify`
    /// event. Separate from `sha` on purpose: an `attach` refreshes liveness
    /// and must not refresh this, or the pre-commit sweep would silently mark
    /// every claim it touched as re-read.
    pub verified_sha: Option<String>,
    /// What that verification found. `None` when the anchor has never been
    /// verified.
    pub verified: Option<Outcome>,
    /// The content sha at the **first** attach — what the finding was written
    /// against.
    ///
    /// First, not latest, and that distinction is load-bearing. A claim that
    /// has never been verified is measured against the content it was made
    /// about; if this followed the latest attach instead, the pre-commit
    /// ritual's bulk attach would move the reference to the current file and
    /// silently clear the claim's unverified mark — which is the failure the
    /// separate `Verify` event exists to prevent.
    pub origin_sha: Option<String>,
}

/// A finding's anchor-log path: `<dir>/<id>.anchor.jsonl`.
fn log_path(dir: &Path, finding_id: &str) -> PathBuf {
    dir.join(format!("{finding_id}.anchor.jsonl"))
}

/// Append one event to `finding_id`'s anchor log (creating it if absent),
/// one JSON object per line. `O_APPEND` keeps concurrent appends from
/// interleaving within a line; the line is fsynced for durability.
pub(super) fn append_event(
    dir: &Path,
    finding_id: &str,
    event: &AnchorEvent,
) -> Result<(), DbError> {
    let line = serde_json::to_string(event)
        .map_err(|e| DbError::Backend(format!("serialize anchor event: {e}")))?;
    let path = log_path(dir, finding_id);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(DbError::Io)?;
    writeln!(file, "{line}").map_err(DbError::Io)?;
    file.sync_all().map_err(DbError::Io)?;
    Ok(())
}

/// Read `finding_id`'s anchor log. A missing log is an empty history; an
/// unparseable line is skipped with a warning rather than failing the read.
pub(super) fn read_log(dir: &Path, finding_id: &str) -> Result<Vec<AnchorEvent>, DbError> {
    let path = log_path(dir, finding_id);
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(DbError::Io(e)),
    };
    let mut events = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<AnchorEvent>(line) {
            Ok(ev) => events.push(ev),
            Err(e) => {
                tracing::warn!(
                    "skipping unparseable anchor event in {}: {e}",
                    path.display()
                );
            }
        }
    }
    Ok(events)
}

/// Fold a log into the current anchor set with per-anchor liveness. Order
/// matters: `Attach` adds a path and bumps its recency/count, `Rename` carries
/// liveness to the new path, `Detach` removes it. Output is sorted by path
/// (`BTreeMap`) for deterministic results.
#[must_use]
pub(super) fn fold(events: &[AnchorEvent]) -> Vec<Anchor> {
    use std::collections::BTreeMap;
    // path -> (recency = max attach ts, attach_count, most-recent-attach sha,
    //          last-verify sha, last-verify outcome)
    type Fold = (
        Timestamp,
        u32,
        Option<String>,
        Option<String>,
        Option<Outcome>,
        Option<String>,
    );
    let mut set: BTreeMap<String, Fold> = BTreeMap::new();
    for ev in events {
        match ev {
            AnchorEvent::Attach { anchor, ts, sha } => {
                let entry = set
                    .entry(anchor.clone())
                    .or_insert((*ts, 0, None, None, None, None));
                // Recency is a max over ts, so the sha of the latest attach wins.
                if *ts >= entry.0 {
                    entry.2.clone_from(sha);
                }
                entry.0 = entry.0.max(*ts);
                entry.1 = entry.1.saturating_add(1);
                // First attach only: the content the finding was written
                // against. A later attach must not move it.
                if entry.5.is_none() {
                    entry.5.clone_from(sha);
                }
            }
            AnchorEvent::Rename { from, to, .. } => {
                if let Some((rec, cnt, sha, vsha, vout, osha)) = set.remove(from) {
                    match set.get_mut(to) {
                        Some(existing) => {
                            if rec >= existing.0 {
                                existing.2 = sha;
                                // Verification travels with the content, like
                                // liveness: a moved file was not re-read.
                                existing.3 = vsha;
                                existing.4 = vout;
                                existing.5 = osha;
                            }
                            existing.0 = existing.0.max(rec);
                            existing.1 = existing.1.saturating_add(cnt);
                        }
                        None => {
                            set.insert(to.clone(), (rec, cnt, sha, vsha, vout, osha));
                        }
                    }
                }
            }
            AnchorEvent::Detach { anchor, .. } => {
                set.remove(anchor);
            }
            AnchorEvent::Verify {
                anchor,
                ts,
                sha,
                outcome,
            } => {
                // Records the verification WITHOUT touching `sha` (field 2).
                // That separation is the whole point: an attach refreshes
                // liveness, a verify refreshes verification, and neither stands
                // in for the other.
                let entry = set
                    .entry(anchor.clone())
                    .or_insert((*ts, 0, None, None, None, None));
                entry.3.clone_from(sha);
                entry.4 = Some(*outcome);
            }
        }
    }
    set.into_iter()
        .map(
            |(path, (recency, attach_count, sha, verified_sha, verified, origin_sha))| Anchor {
                path,
                recency,
                attach_count,
                sha,
                verified_sha,
                verified,
                origin_sha,
            },
        )
        .collect()
}

/// Seed `successor`'s anchor log from `predecessor`'s current anchor set:
/// append an `Attach` (at `ts`) for each anchor the predecessor currently has.
/// A correction (`supersedes:`) thus inherits the superseded finding's
/// reachability — otherwise the superseding finding, preferred by retrieval,
/// would have no anchors and be unreachable by `find_directives`.
pub(super) fn seed_from_predecessor(
    dir: &Path,
    predecessor: &str,
    successor: &str,
    ts: Timestamp,
) -> Result<(), DbError> {
    for anchor in fold(&read_log(dir, predecessor)?) {
        append_event(
            dir,
            successor,
            &AnchorEvent::Attach {
                anchor: anchor.path,
                ts,
                sha: anchor.sha,
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::{append_event, fold, read_log, seed_from_predecessor, Anchor, AnchorEvent};
    use crate::clock::Timestamp;

    /// A `Timestamp` at `secs` Unix seconds (test fixture).
    pub(super) fn ts(secs: i64) -> Timestamp {
        time::OffsetDateTime::from_unix_timestamp(secs)
            .unwrap()
            .into()
    }

    fn attach(p: &str, secs: i64) -> AnchorEvent {
        AnchorEvent::Attach {
            anchor: p.to_owned(),
            ts: ts(secs),
            sha: None,
        }
    }

    fn attach_sha(p: &str, secs: i64, sha: &str) -> AnchorEvent {
        AnchorEvent::Attach {
            anchor: p.to_owned(),
            ts: ts(secs),
            sha: Some(sha.to_owned()),
        }
    }

    #[test]
    fn fold_collects_set_with_recency_and_count() {
        let events = vec![
            attach("a/b.rs", 100),
            attach("a/b.rs", 200), // re-attach: bumps recency + count
            attach("c/", 150),
        ];
        let anchors = fold(&events);
        assert_eq!(
            anchors,
            vec![
                Anchor {
                    path: "a/b.rs".to_owned(),
                    recency: ts(200),
                    attach_count: 2,
                    sha: None,
                    verified_sha: None,
                    verified: None,
                    origin_sha: None,
                },
                Anchor {
                    path: "c/".to_owned(),
                    recency: ts(150),
                    attach_count: 1,
                    sha: None,
                    verified_sha: None,
                    verified: None,
                    origin_sha: None,
                },
            ]
        );
    }

    #[test]
    fn rename_carries_liveness_and_detach_removes() {
        let events = vec![
            attach("old.rs", 100),
            AnchorEvent::Rename {
                from: "old.rs".to_owned(),
                to: "new.rs".to_owned(),
                ts: ts(110),
            },
            attach("gone/", 120),
            AnchorEvent::Detach {
                anchor: "gone/".to_owned(),
                ts: ts(130),
            },
        ];
        let anchors = fold(&events);
        assert_eq!(
            anchors,
            vec![Anchor {
                path: "new.rs".to_owned(),
                recency: ts(100),
                attach_count: 1,
                sha: None,
                verified_sha: None,
                verified: None,
                origin_sha: None,
            }]
        );
    }

    #[test]
    fn fold_carries_latest_attach_sha_and_rename_moves_it() {
        // Latest attach's sha wins; rename carries it to the new path.
        let events = vec![
            attach_sha("f.rs", 100, "aaaa"),
            attach_sha("f.rs", 200, "bbbb"),
            AnchorEvent::Rename {
                from: "f.rs".to_owned(),
                to: "g.rs".to_owned(),
                ts: ts(210),
            },
        ];
        let anchors = fold(&events);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].path, "g.rs");
        assert_eq!(anchors[0].sha.as_deref(), Some("bbbb"));
    }

    #[test]
    fn fold_sha_is_none_for_sha_less_log() {
        // A log predating shas folds to None — drift unknown, treated as live.
        let anchors = fold(&[attach("f.rs", 100)]);
        assert_eq!(anchors[0].sha, None);
    }

    #[test]
    fn attach_event_serde_omits_absent_sha_and_round_trips_present() {
        // Old line (no sha key) deserializes to None — no migration.
        let old: AnchorEvent =
            serde_json::from_str(r#"{"op":"attach","anchor":"f.rs","ts":"1970-01-01T00:00:00Z"}"#)
                .unwrap();
        assert_eq!(old, attach("f.rs", 0));
        // A None sha is omitted from the serialized line.
        let line = serde_json::to_string(&attach("f.rs", 0)).unwrap();
        assert!(!line.contains("sha"), "absent sha omitted: {line}");
        // A present sha round-trips.
        let with = attach_sha("f.rs", 0, "deadbeef");
        assert_eq!(
            serde_json::from_str::<AnchorEvent>(&serde_json::to_string(&with).unwrap()).unwrap(),
            with
        );
    }

    #[test]
    fn append_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        append_event(dir.path(), "fnd_a", &attach("f.rs", 100)).unwrap();
        append_event(dir.path(), "fnd_a", &attach("f.rs", 200)).unwrap();
        let events = read_log(dir.path(), "fnd_a").unwrap();
        assert_eq!(events, vec![attach("f.rs", 100), attach("f.rs", 200)]);
        // A finding with no log folds to an empty anchor set.
        assert!(read_log(dir.path(), "fnd_missing").unwrap().is_empty());
    }

    #[test]
    fn supersede_seeds_successor_from_predecessor() {
        let dir = tempfile::tempdir().unwrap();
        append_event(dir.path(), "fnd_old", &attach("a.rs", 100)).unwrap();
        append_event(dir.path(), "fnd_old", &attach("b/", 110)).unwrap();

        seed_from_predecessor(dir.path(), "fnd_old", "fnd_new", ts(200)).unwrap();

        let new_anchors: Vec<String> = fold(&read_log(dir.path(), "fnd_new").unwrap())
            .into_iter()
            .map(|a| a.path)
            .collect();
        assert_eq!(new_anchors, vec!["a.rs".to_owned(), "b/".to_owned()]);
    }

    #[test]
    fn read_skips_unparseable_lines() {
        let dir = tempfile::tempdir().unwrap();
        append_event(dir.path(), "fnd_a", &attach("f.rs", 100)).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join("fnd_a.anchor.jsonl"))
            .unwrap()
            .write_all(b"{ not json\n")
            .unwrap();
        let events = read_log(dir.path(), "fnd_a").unwrap();
        assert_eq!(events, vec![attach("f.rs", 100)]);
    }
}

#[cfg(test)]
mod verification_tests {
    use super::{fold, Anchor, AnchorEvent, Outcome};

    fn attach_sha(path: &str, ts: i64, sha: &str) -> AnchorEvent {
        AnchorEvent::Attach {
            anchor: path.to_owned(),
            ts: super::tests::ts(ts),
            sha: Some(sha.to_owned()),
        }
    }

    fn verify(path: &str, ts: i64, sha: &str, outcome: Outcome) -> AnchorEvent {
        AnchorEvent::Verify {
            anchor: path.to_owned(),
            ts: super::tests::ts(ts),
            sha: Some(sha.to_owned()),
            outcome,
        }
    }

    fn only(anchors: Vec<Anchor>) -> Anchor {
        assert_eq!(anchors.len(), 1, "one anchor");
        anchors.into_iter().next().expect("one")
    }

    #[test]
    fn an_attach_does_not_record_a_verification() {
        // The rule this whole change exists for, made structural: `attach` and
        // `verify` write different fields, so no amount of attaching can
        // declare a claim read. The pre-commit ritual attaches in bulk.
        let a = only(fold(&[attach_sha("f.rs", 100, "aaa")]));
        assert_eq!(a.sha.as_deref(), Some("aaa"), "liveness recorded");
        assert_eq!(a.verified_sha, None, "but nothing was verified");
        assert_eq!(a.verified, None);
    }

    #[test]
    fn a_later_attach_does_not_move_the_verification_point() {
        // The failure mode in full: verify at one sha, the file changes, the
        // ritual re-attaches. The claim must still count as unverified against
        // the NEW content, so the verification sha must not follow the attach.
        let a = only(fold(&[
            attach_sha("f.rs", 100, "aaa"),
            verify("f.rs", 110, "aaa", Outcome::StillTrue),
            attach_sha("f.rs", 200, "bbb"),
        ]));
        assert_eq!(a.sha.as_deref(), Some("bbb"), "liveness followed the file");
        assert_eq!(
            a.verified_sha.as_deref(),
            Some("aaa"),
            "verification stayed where someone actually read it"
        );
    }

    #[test]
    fn the_latest_verification_wins_and_carries_its_outcome() {
        let a = only(fold(&[
            attach_sha("f.rs", 100, "aaa"),
            verify("f.rs", 110, "aaa", Outcome::StillTrue),
            verify("f.rs", 200, "bbb", Outcome::PartlyTrue),
        ]));
        assert_eq!(a.verified_sha.as_deref(), Some("bbb"));
        assert_eq!(a.verified, Some(Outcome::PartlyTrue));
    }

    #[test]
    fn a_rename_carries_verification_with_the_content() {
        // A moved file was not re-read, so its verification travels with it —
        // dropping it would report every renamed claim as freshly unverified.
        let a = only(fold(&[
            attach_sha("old.rs", 100, "aaa"),
            verify("old.rs", 110, "aaa", Outcome::StillTrue),
            AnchorEvent::Rename {
                from: "old.rs".into(),
                to: "new.rs".into(),
                ts: super::tests::ts(120),
            },
        ]));
        assert_eq!(a.path, "new.rs");
        assert_eq!(a.verified_sha.as_deref(), Some("aaa"));
        assert_eq!(a.verified, Some(Outcome::StillTrue));
    }

    #[test]
    fn every_outcome_round_trips_through_the_log_format() {
        for o in [
            Outcome::StillTrue,
            Outcome::NoLongerTrue,
            Outcome::PartlyTrue,
        ] {
            let ev = verify("f.rs", 1, "aaa", o);
            let line = serde_json::to_string(&ev).expect("serialize");
            assert!(line.contains("\"op\":\"verify\""), "{line}");
            let back: AnchorEvent = serde_json::from_str(&line).expect("round trip");
            assert_eq!(back, ev);
        }
    }
}
