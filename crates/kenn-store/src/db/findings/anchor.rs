//! Anchor + liveness event logs — `.kenn/findings/<id>.anchor.jsonl`.
//!
//! A finding's *anchors* (the files/dirs it applies to) and their liveness are a
//! per-finding append-only event log, kept separate from the immutable
//! `<id>.md` record because file/dir paths get moved, renamed, and deleted.
//! Event kinds are `attach`, `rename`, and `detach`; a repeat `attach` to a path
//! already in the set is the liveness signal (there is no separate confirm
//! event). The current anchor set and per-anchor liveness are a fold over the
//! log. One JSON object per line — appends from two branches union with no
//! conflict on `git merge`.
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
    // path -> (recency = max attach ts, attach_count, most-recent-attach sha)
    let mut set: BTreeMap<String, (Timestamp, u32, Option<String>)> = BTreeMap::new();
    for ev in events {
        match ev {
            AnchorEvent::Attach { anchor, ts, sha } => {
                let entry = set.entry(anchor.clone()).or_insert((*ts, 0, None));
                // Recency is a max over ts, so the sha of the latest attach wins.
                if *ts >= entry.0 {
                    entry.2.clone_from(sha);
                }
                entry.0 = entry.0.max(*ts);
                entry.1 = entry.1.saturating_add(1);
            }
            AnchorEvent::Rename { from, to, .. } => {
                if let Some((rec, cnt, sha)) = set.remove(from) {
                    match set.get_mut(to) {
                        Some(existing) => {
                            if rec >= existing.0 {
                                existing.2 = sha;
                            }
                            existing.0 = existing.0.max(rec);
                            existing.1 = existing.1.saturating_add(cnt);
                        }
                        None => {
                            set.insert(to.clone(), (rec, cnt, sha));
                        }
                    }
                }
            }
            AnchorEvent::Detach { anchor, .. } => {
                set.remove(anchor);
            }
        }
    }
    set.into_iter()
        .map(|(path, (recency, attach_count, sha))| Anchor {
            path,
            recency,
            attach_count,
            sha,
        })
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
    fn ts(secs: i64) -> Timestamp {
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
                },
                Anchor {
                    path: "c/".to_owned(),
                    recency: ts(150),
                    attach_count: 1,
                    sha: None,
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
