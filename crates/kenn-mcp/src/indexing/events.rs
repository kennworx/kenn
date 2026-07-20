//! Hot-reload + freshness machinery: the watcher live-event handler, the
//! staleness backstop poll, the synthetic-event seed, and the
//! recovery / background-reindex spawners.

use std::sync::Arc;

use kenn_indexer::pipeline::ProgressEvent;
use tokio::sync::mpsc;

use crate::state::{LifecycleState, ProgressSnapshot};
use crate::tools::ServerState;

use super::{reload_kenn_toml, run_startup_decision, swap_to_snapshot};

/// Hot-reload driven by the watcher's `live`-pointer event (D3). Resolve
/// `live`; if it equals the served run, no-op (self-publish dedup); else
/// cross-instance swap, stamping `run_event_seq := event_seq()` ("caught
/// up as of now", D4). Public (doc-hidden) so integration tests can
/// drive a cross-instance reload deterministically.
#[doc(hidden)]
pub async fn handle_live_event(state: &ServerState, git_aware_skip: bool, config_sig: u64) {
    let current = {
        let g = state.lifecycle.read().expect("lifecycle lock poisoned");
        match &*g {
            LifecycleState::Ready { snapshot_path, .. } => Some(snapshot_path.clone()),
            _ => None,
        }
    };
    let Some(current) = current else { return };

    let store = match kenn_store::Store::open(state.layout()) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("kenn-mcp: live-event hot-reload skipped, store unavailable: {e}");
            return;
        }
    };
    let Some(live) = store.live_target() else {
        return;
    };
    if live == current {
        // Self-publish dedup: our own reindex already swapped to this run
        // via its self-publish swap. No-op.
        return;
    }
    let run_seq = state.event_seq();
    swap_to_snapshot(state, &store, &live, run_seq, git_aware_skip, config_sig).await;
}

/// Test-only compatibility shim: drive one cross-instance hot-reload —
/// the same effect the watcher's `live` event produces in production.
/// The `layout` argument is ignored (the layout is read from `state`).
#[doc(hidden)]
pub async fn poll_once(
    state: &ServerState,
    _layout: &kenn_store::Layout,
    git_aware_skip: bool,
    config_sig: u64,
) {
    handle_live_event(state, git_aware_skip, config_sig).await;
}

/// Synthesize an event (D4/D5 bridge): bump `last_event_seq` so
/// `is_stale` flips, AND invoke the reindex trigger directly. Invoked by
/// the startup seed and the backstop — independent of the notify
/// watcher's liveness, so it still works after `watch_stop`.
fn synthesize_event(state: &Arc<ServerState>) {
    state.bump_event_seq();
    spawn_background_reindex(Arc::clone(state));
}

/// Startup seed (D4): one `spawn_blocking` git key-compare against the
/// served run, reconciling a change made while the server was down. On
/// stale it synthesizes an event. Off the read path. Best-effort: an
/// unreadable key leaves `is_stale` optimistically false (the backstop
/// retries).
pub(crate) fn startup_seed(state: &Arc<ServerState>) {
    let state = Arc::clone(state);
    let config_sig = state.config.indexing_signature();
    drop(tokio::task::spawn_blocking(move || {
        if backstop_is_stale(&state, config_sig) {
            synthesize_event(&state);
        }
    }));
}

/// Low-frequency git staleness backstop (D5): at `backstop_secs`
/// cadence, run a key-compare on `spawn_blocking`; on mismatch
/// synthesize an event — catching dropped watcher events, and (after
/// `watch_stop`) serving as the only freshness mechanism. Each tick also
/// reconciles the served reader against `live` as a safety net for a
/// missed `live` event. `0` disables the backstop.
///
/// Limitation (accepted, D5): the git staleness key cannot see
/// gitignored generated files — the backstop is a floor for missed
/// watcher events on *tracked* files only. Gitignored source is covered
/// by the watcher's path-based filter (which does not consult git), not
/// by this backstop.
pub fn start_staleness_backstop_task(
    state: Arc<ServerState>,
    git_aware_skip: bool,
    config_sig: u64,
    backstop_secs: u64,
) {
    if backstop_secs == 0 {
        tracing::info!("kenn-mcp: staleness backstop disabled (staleness_backstop_secs=0)");
        return;
    }
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(backstop_secs));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await; // discard the immediate first tick
        loop {
            ticker.tick().await;
            // Safety net for a missed `live` event (e.g. another instance
            // published and our watcher dropped the event).
            handle_live_event(&state, git_aware_skip, config_sig).await;
            // Git key-compare on a blocking thread; synthesize an event
            // on mismatch so `is_stale` flips and a reindex fires.
            let state_for_check = Arc::clone(&state);
            let key_mismatch = tokio::task::spawn_blocking(move || {
                backstop_is_stale(&state_for_check, config_sig)
            })
            .await
            .unwrap_or(false);
            if key_mismatch {
                synthesize_event(&state);
            }
        }
    });
}

/// Compute the workspace's current git staleness key and compare it to
/// the served run's recorded key. `true` when they differ. Runs git +
/// file hashing — callers MUST invoke it on a blocking thread, never the
/// dispatch runtime (D1/D5). Returns `false` when the key can't be read
/// ("unknown" → not-stale; the next tick retries).
fn backstop_is_stale(state: &ServerState, config_sig: u64) -> bool {
    let current = {
        let g = state.lifecycle.read().expect("lifecycle lock poisoned");
        match &*g {
            LifecycleState::Ready { snapshot_path, .. } => snapshot_path.clone(),
            _ => return false,
        }
    };
    let Some(recorded) = read_recorded_staleness_key(&current) else {
        return false;
    };
    let now = kenn_store::staleness::compute_staleness_key(&state.source_root(), config_sig);
    !now.matches(&recorded)
}

pub(crate) fn set_failed(state: &ServerState, error: String) {
    let now = std::time::Instant::now();
    let mut g = state.lifecycle.write().expect("lifecycle lock poisoned");
    let started_at = match &*g {
        LifecycleState::Indexing { started_at, .. } => *started_at,
        _ => now,
    };
    *g = LifecycleState::Failed {
        error,
        started_at,
        ended_at: now,
    };
}

fn read_recorded_staleness_key(
    snapshot_dir: &std::path::Path,
) -> Option<kenn_store::staleness::StalenessKey> {
    use serde::Deserialize;
    #[derive(Deserialize)]
    struct MetaStaleness {
        staleness_key: Option<kenn_store::staleness::StalenessKey>,
    }
    let bytes = std::fs::read(snapshot_dir.join("meta.json")).ok()?;
    let parsed: MetaStaleness = serde_json::from_slice(&bytes).ok()?;
    parsed.staleness_key
}

/// Spawn the recovery pipeline after the `reindex` tool transitioned
/// the lifecycle from `Failed` → `Indexing`. Reuses the cold-start path
/// directly; on success the server reaches `Ready`. The progress channel
/// is local (no notification pump) — recovery is rare and the call site
/// surfaces final status via `get_index_status`.
pub fn spawn_recovery_pipeline(state: Arc<ServerState>) {
    let layout = state.layout();
    let config = reload_kenn_toml(&layout, &state.config);
    tokio::spawn(async move {
        let (tx, _rx) = mpsc::unbounded_channel::<ProgressEvent>();
        run_startup_decision(&state, &layout, &config, &tx).await;
    });
}

/// Spawn a background reindex while the server stays `Ready` on its
/// current snapshot. The poll task swaps to the new snapshot on success;
/// failures clear `Ready.reindex` and leave the prior snapshot in
/// service (Decision 5) — `Failed` is only reachable from cold-start.
///
/// Cross-instance coordination is automatic: `index_workspace` calls
/// `begin_indexing`, which `flock`s `index.lock`. If another instance
/// (or a `kenn index` CLI run) already holds it, the workflow returns
/// `WorkflowError::Begin(BeginError::LockHeld(_))` — this routine
/// coalesces silently (try-lock-and-bail, D6) and reloads the winner's
/// snapshot via the `live` watch.
pub fn spawn_background_reindex(state: Arc<ServerState>) {
    let layout = state.layout();
    let config = reload_kenn_toml(&layout, &state.config);
    let git_aware_skip = config.staleness.git_aware_skip;
    let config_sig = config.indexing_signature();
    tokio::spawn(async move {
        // Capture the event-seq at the reindex's START (D4). On a
        // self-publish swap below this becomes the served run's
        // `run_event_seq`, so any event landing mid-reindex still leaves
        // `is_stale` true (no lost update).
        let start_seq = state.event_seq();
        let progress = {
            let lifecycle = state.lifecycle.clone();
            move |ev: ProgressEvent| {
                if let Ok(mut g) = lifecycle.write() {
                    if let LifecycleState::Ready {
                        reindex: Some(r), ..
                    } = &mut *g
                    {
                        let snap = r.progress.get_or_insert_with(ProgressSnapshot::default);
                        snap.observe(&ev);
                    }
                }
            }
        };
        let result = kenn_indexer::index_workspace(
            &layout,
            &config,
            progress,
            kenn_analyze::analysis_hook_from_config(&config),
        )
        .await;

        // Clear `Ready.reindex` regardless of outcome. Background-reindex
        // failures never reach `Failed` (Decision 5) — the prior reader
        // stays in service.
        if let Ok(mut g) = state.lifecycle.write() {
            if let LifecycleState::Ready { reindex, .. } = &mut *g {
                *reindex = None;
            }
        }

        match result {
            Ok(outcome) => {
                // Self-publish swap (D4): swap to our own just-published
                // run, stamping `run_event_seq` with the counter captured
                // at this reindex's start. Done here (not via a timer);
                // our own resulting `live` event is then deduped by
                // `handle_live_event`.
                match kenn_store::Store::open(layout.clone()) {
                    Ok(store) => {
                        swap_to_snapshot(
                            &state,
                            &store,
                            &outcome.snapshot_path,
                            start_seq,
                            git_aware_skip,
                            config_sig,
                        )
                        .await;
                    }
                    Err(e) => tracing::warn!(
                        "kenn-mcp: reindex published but store reopen failed: {e}; \
                         the `live` watch / backstop will reload"
                    ),
                }
            }
            Err(e)
                if matches!(
                    &e,
                    kenn_indexer::workflow::WorkflowError::Begin(kenn_store::BeginError::LockHeld(
                        _
                    ))
                ) =>
            {
                tracing::info!(
                    "kenn-mcp: reindex coalesced — another writer holds index.lock; \
                     the `live` watch will reload the winner's snapshot"
                );
            }
            Err(e) => {
                tracing::warn!(
                    "kenn-mcp: background reindex failed: {e}; staying on prior snapshot"
                );
            }
        }
    });
}
