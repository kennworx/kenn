//! Operator-driven lifecycle actions — `rollback` to the previous
//! published run and `gc` to evict cold ones.

use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::layout::{RunMeta, Store};

use super::atomic::atomic_flip_live;
use super::state::{access_time, list_completed_runs};
use super::types::RollbackError;

/// Roll back to the previously-published run. The currently-live run
/// becomes the new "previous" and remains retained.
pub fn rollback(store: &Store) -> Result<PathBuf, RollbackError> {
    let live_target = store.live_target().ok_or(RollbackError::NoLive)?;
    let runs = list_completed_runs(store);

    // Find the run that comes before `live` lexicographically — the
    // most recent run strictly older than the current `live` target.
    let live_name = live_target
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(RollbackError::NoLive)?;
    let prev = runs
        .iter()
        .rfind(|r| r.id.as_str() < live_name)
        .ok_or(RollbackError::NoPrevious)?;

    atomic_flip_live(&store.live_path(), &prev.path)?;
    Ok(prev.path.clone())
}

/// Garbage-collect published runs by least-recently-used: retain the
/// `retention` (`[lifecycle] gc_keep`) most-recently-*accessed* runs,
/// evict the rest.
///
/// Access time is the `.accessed` marker mtime
/// ([`super::state::access_time`]), which run resolution refreshes on
/// every `Skip`. Retention is *not* keyed on the staleness key — that
/// key changes on every edit, so a per-key policy would grow without
/// bound.
///
/// The current `live` target is exempt — never evicted regardless of
/// its LRU position — so `rollback` always retains a target even
/// after a rollback drops `live` onto a cold run.
///
/// A run held open by any process's reader (registered in the
/// `<derived_root>/readers/` registry — see [`crate::readers`]) is
/// also exempt, so a long-running MCP server in another process
/// never has a run deleted out from under it. The pin probe is
/// non-blocking `flock`, so a crashed reader does not leak a
/// permanent pin. Probe errors are treated conservatively as
/// "pinned."
///
/// Only **published** runs (with `meta.json`) are considered. Runs
/// without `meta.json` are incomplete and get swept by
/// [`super::recover::recover`].
pub fn gc(store: &Store, retention: usize) -> Result<Vec<PathBuf>, std::io::Error> {
    let retention = retention.max(1);
    let live = store.live_target();
    let runs = list_completed_runs(store);

    let mut scored: Vec<(SystemTime, RunMeta)> = runs
        .into_iter()
        .map(|m| (access_time(&m.path), m))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.id.cmp(&a.1.id)));

    let mut deleted = Vec::new();
    for (rank, (_, run)) in scored.into_iter().enumerate() {
        if rank < retention {
            continue;
        }
        if live.as_deref() == Some(run.path.as_path()) {
            continue;
        }
        if crate::readers::snapshot_has_live_reader(store, &run.path).unwrap_or(true) {
            continue;
        }
        fs::remove_dir_all(&run.path)?;
        deleted.push(run.path);
    }
    Ok(deleted)
}
