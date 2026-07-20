//! Read-only views over the on-disk lifecycle state — `current_state`,
//! `list_completed_runs`, `decide_startup_state`, and the access-time
//! plumbing that drives LRU [`crate::lifecycle::gc`].

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::layout::{RunMeta, Store};

use super::types::{LifecycleState, StartupDecision, ACCESS_MARKER, META_FILE};

/// Read the current state from disk.
#[must_use]
pub fn current_state(store: &Store) -> LifecycleState {
    let live = store.live_target();
    let incomplete = incomplete_runs(store);
    match (live, incomplete.first()) {
        (None, None) => LifecycleState::Uninitialized,
        (Some(live), None) => LifecycleState::Steady { live },
        (live, Some(run_dir)) => LifecycleState::Indexing {
            live,
            run_dir: run_dir.clone(),
        },
    }
}

/// Decide whether a new caller should immediately serve an existing
/// published run or trigger a fresh indexing pass. Centralizes the
/// "is a run fresh enough" judgement so MCP and CLI agree.
///
/// - `git_aware_skip`: the user's `kenn.toml` `staleness.git_aware_skip`
///   setting. When false, this returns `Skip` whenever a servable `live`
///   symlink exists, deferring all *freshness* judgement to the caller
///   (schema/backend compatibility is still enforced — see below).
/// - When `git_aware_skip` is true: compute the workspace's current
///   `StalenessKey` and scan **every retained published run** under
///   the derived store for one whose recorded key matches. A match ⇒
///   `Skip` that run; no match ⇒ `Reindex`. Scanning the whole set
///   lets a derived store shared across branches or worktrees serve
///   each from its own matching run.
/// - `config_sig`: the indexing-affecting config hash
///   ([`kenn_config::Config::indexing_signature`]) folded into the
///   current key, so a changed `[language.*]` config never matches a run
///   recorded under the old config — it `Reindex`es.
///
/// In both modes a run is only `Skip`-eligible when it is *servable* by
/// this binary — its recorded `schema_version` and `backend` match the
/// compiled-in values. A run built under an older schema can't be
/// opened, so an unchanged workspace must still `Reindex` rather than
/// loop forever serving a snapshot that fails to open (the schema-bump
/// case).
///
/// Conservative on errors: an unreadable recorded key is treated as a
/// non-match. The cost of a redundant reindex is bounded; the cost of
/// serving stale data is not. A `Skip` decision touches the selected
/// run's access time so LRU GC keeps it hot.
#[must_use]
pub fn decide_startup_state(
    store: &Store,
    workspace_root: &Path,
    git_aware_skip: bool,
    config_sig: u64,
) -> StartupDecision {
    if !git_aware_skip {
        return follow_live(store);
    }

    let current = crate::staleness::compute_staleness_key(workspace_root, config_sig);
    let runs = list_completed_runs(store);
    // Prefer the most recent match — `list_completed_runs` sorts ascending.
    for run in runs.into_iter().rev() {
        if !run_is_servable(&run.path) {
            continue;
        }
        if let Some(recorded) = read_recorded_staleness_key(&run.path) {
            if current.matches(&recorded) {
                touch_access(&run.path);
                return StartupDecision::Skip { live: run.path };
            }
        }
    }
    StartupDecision::Reindex {
        reason: "no retained run matches the workspace",
    }
}

/// Serve the `live` run if one exists and is servable by this binary,
/// else `Reindex`. The decision when freshness should not be judged —
/// `git_aware_skip` is off — but schema/backend compatibility still
/// gates `Skip`, since an unopenable snapshot is never servable.
fn follow_live(store: &Store) -> StartupDecision {
    match store.live_target() {
        Some(live) if run_is_servable(&live) => {
            touch_access(&live);
            StartupDecision::Skip { live }
        }
        Some(_) => StartupDecision::Reindex {
            reason: "live run schema/backend mismatch",
        },
        None => StartupDecision::Reindex {
            reason: "no live run",
        },
    }
}

/// A published run is servable only when its on-disk format matches this
/// binary: both the recorded `schema_version` and `backend` marker agree
/// with the compiled-in values. A run that fails either check can't be
/// opened by [`open_reader`](crate::open_reader), so it must not be
/// `Skip`-served — it falls through to `Reindex` even when the workspace
/// source is unchanged.
fn run_is_servable(run_dir: &Path) -> bool {
    crate::meta::check_schema_version(run_dir).is_ok()
        && crate::meta::check_backend_marker(run_dir).is_ok()
}

/// Record `run_dir` as accessed now — rewrite its `.accessed` marker
/// so LRU [`crate::lifecycle::gc`] keeps it hot. Best-effort: a failure
/// leaves GC to fall back to the run directory's own mtime.
pub(super) fn touch_access(run_dir: &Path) {
    drop(fs::write(run_dir.join(ACCESS_MARKER), b""));
}

/// A run's last-access time: its `.accessed` marker mtime, falling
/// back to the run directory's mtime (≈ publish time) when no marker
/// exists, and to the epoch when even that is unreadable.
pub(super) fn access_time(run_dir: &Path) -> SystemTime {
    fs::metadata(run_dir.join(ACCESS_MARKER))
        .and_then(|m| m.modified())
        .or_else(|_| fs::metadata(run_dir).and_then(|m| m.modified()))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Read the `staleness_key` field out of a run's `meta.json`.
/// Tolerates additional unknown fields — only the staleness key is
/// extracted. Returns `None` if the file is missing, unreadable, or
/// the JSON does not contain a parseable `staleness_key`.
fn read_recorded_staleness_key(run_dir: &Path) -> Option<crate::staleness::StalenessKey> {
    use serde::Deserialize;
    #[derive(Deserialize)]
    struct MetaStaleness {
        staleness_key: Option<crate::staleness::StalenessKey>,
    }
    let meta_path = run_dir.join(META_FILE);
    let bytes = fs::read(&meta_path).ok()?;
    let parsed: MetaStaleness = serde_json::from_slice(&bytes).ok()?;
    parsed.staleness_key
}

/// Run directories under `runs/` that are missing the `meta.json`
/// completion stamp — incomplete or crashed runs. Empty when the
/// `runs/` directory is absent or every run is complete.
pub(super) fn incomplete_runs(store: &Store) -> Vec<PathBuf> {
    store
        .list_runs()
        .unwrap_or_default()
        .into_iter()
        .filter(|m| !m.path.join(META_FILE).is_file())
        .map(|m| m.path)
        .collect()
}

/// Published runs only — those carrying a `meta.json` completion
/// stamp. Sorted ascending by id (ISO-8601 timestamps sort
/// lexicographically). The `Vec<RunMeta>` shape mirrors
/// `Store::list_runs`; this helper just applies the
/// completed-vs-incomplete filter.
#[must_use]
pub fn list_completed_runs(store: &Store) -> Vec<RunMeta> {
    store
        .list_runs()
        .unwrap_or_default()
        .into_iter()
        .filter(|m| m.path.join(META_FILE).is_file())
        .collect()
}
