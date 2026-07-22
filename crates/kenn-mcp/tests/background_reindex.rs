//! Integration tests for `mcp-background-reindex` — hot-reload,
//! reindex tool, real status, and multi-instance correctness.
//!
//! The tests skip the actual indexer pipeline (slow, needs language
//! toolchains) and publish empty snapshots through
//! `kenn_store::lifecycle::begin_indexing` + `publish`. Hot-reload is
//! driven by calling [`kenn_mcp::indexing::poll_once`] directly so each
//! test runs synchronously rather than waiting for the ~3 s timer.
//!
//! The actual cold-start pipeline + cross-instance reindex coalescing
//! against a *running* `kenn index` are validated at apply time via the
//! CLI smoke tests — running them in-process here would need the
//! language indexers, which is out of scope for unit-level tests.

use std::time::Instant;

use kenn_mcp::indexing::poll_once;
use kenn_mcp::state::{LifecycleState, ProgressSnapshot, ReindexProgress};
use kenn_mcp::tools::{self, GetIndexStatusArgs, ReindexArgs};
use kenn_store::{Layout, Store};
use tempfile::TempDir;

mod common;
use common::{make_state, place_ready, publish_snapshot, served_snapshot};

// ── tests ──────────────────────────────────────────────────────────────────

/// Task 3.7: a newer snapshot published by another process is detected
/// by the poll task and the server's reader atomically swaps to it.
#[tokio::test]
async fn external_publish_is_hot_reloaded() {
    let ws = TempDir::new().unwrap();
    let snap_a = publish_snapshot(ws.path()).await;
    let state = make_state(ws.path());
    place_ready(&state, &snap_a).await;
    assert_eq!(served_snapshot(&state), snap_a);

    // Another process (simulated) publishes a newer snapshot.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let snap_b = publish_snapshot(ws.path()).await;
    assert_ne!(snap_a, snap_b);

    // Drive one poll tick — should swap.
    poll_once(&state, &state.layout(), false, 0).await;
    assert_eq!(served_snapshot(&state), snap_b);
}

/// Task 3.8 (partial): when a newer snapshot can't be opened, the
/// server keeps serving the current one — the swap is all-or-nothing.
#[tokio::test]
async fn corrupt_newer_snapshot_does_not_blank_server() {
    let ws = TempDir::new().unwrap();
    let snap_a = publish_snapshot(ws.path()).await;
    let state = make_state(ws.path());
    place_ready(&state, &snap_a).await;

    // Forge a "newer" snapshot dir that points `live` somewhere
    // un-openable (an empty directory — `open_reader` should fail
    // because no Lance dataset lives there).
    let store = Store::open(state.layout()).unwrap();
    let bogus = store.runs_dir().join("2099-01-01T00-00-00Z");
    std::fs::create_dir_all(&bogus).unwrap();
    let live = store.live_path();
    drop(std::fs::remove_file(&live)); // OK if it doesn't exist yet
    std::fs::write(&live, "runs/2099-01-01T00-00-00Z").unwrap();

    poll_once(&state, &state.layout(), false, 0).await;
    // Reader stayed on snap_a because opening the bogus dir failed.
    assert_eq!(served_snapshot(&state), snap_a);
}

/// Task 4.6: the `reindex` tool against a `Failed` server transitions
/// it to `Indexing` (recovery retry). We verify the immediate state
/// transition, not the eventual outcome (which depends on the indexer
/// having something to chew on).
#[tokio::test]
async fn failed_state_recovered_by_reindex_call() {
    let ws = TempDir::new().unwrap();
    let state = make_state(ws.path());
    {
        let mut g = state.lifecycle.write().unwrap();
        let now = Instant::now();
        *g = LifecycleState::Failed {
            error: "stub cold-start failure".into(),
            started_at: now,
            ended_at: now,
        };
    }
    let resp = tools::reindex(&state, ReindexArgs::default()).expect("reindex ok");
    assert_eq!(resp.item.unwrap().status, "recovery_started");
    // Immediately after the tool returns, the lifecycle has transitioned
    // to Indexing. (The spawned task may then succeed or fail; we don't
    // wait for it here.)
    let kind = state.lifecycle.read().unwrap().kind();
    assert_eq!(kind, kenn_mcp::state::StateKind::Indexing);
}

/// Task 4.5 (partial, in-process coalescing): a second `reindex` call
/// while a background reindex is already running coalesces — no second
/// task is spawned, the response signals `in_progress`.
#[tokio::test]
async fn reindex_call_coalesces_when_already_in_flight() {
    let ws = TempDir::new().unwrap();
    let snap_a = publish_snapshot(ws.path()).await;
    let state = make_state(ws.path());
    place_ready(&state, &snap_a).await;
    // Pretend a reindex is in flight.
    {
        let mut g = state.lifecycle.write().unwrap();
        if let LifecycleState::Ready { reindex, .. } = &mut *g {
            *reindex = Some(ReindexProgress {
                started_at: Instant::now(),
                progress: None,
            });
        }
    }
    let resp = tools::reindex(&state, ReindexArgs::default()).expect("reindex ok");
    assert_eq!(resp.item.unwrap().status, "in_progress");
}

/// Task 5.3: `get_index_status` reports `is_stale` from the cache, and
/// `reindex_in_progress` plus progress while a reindex is in flight.
#[tokio::test]
async fn status_reports_stale_and_reindex_progress() {
    let ws = TempDir::new().unwrap();
    let snap_a = publish_snapshot(ws.path()).await;
    let state = make_state(ws.path());
    place_ready(&state, &snap_a).await;

    // Idle Ready: neither flag.
    let s0 = tools::get_index_status(&state, GetIndexStatusArgs::default())
        .unwrap()
        .item
        .unwrap();
    assert_eq!(s0.state, "ready");
    assert!(!s0.is_stale);
    assert!(!s0.reindex_in_progress);
    assert!(s0.progress.is_none());

    // Make is_stale true: simulate a watcher event observed since the
    // served run, so the event-seq generation comparison flips (D4).
    state.bump_event_seq();
    // Install a fake in-flight reindex with progress.
    {
        let mut g = state.lifecycle.write().unwrap();
        if let LifecycleState::Ready { reindex, .. } = &mut *g {
            *reindex = Some(ReindexProgress {
                started_at: Instant::now(),
                progress: Some(ProgressSnapshot {
                    phase: "ingest",
                    files_seen: 42,
                    symbols_seen: 0,
                    edges_seen: 0,
                }),
            });
        }
    }
    let s1 = tools::get_index_status(&state, GetIndexStatusArgs::default())
        .unwrap()
        .item
        .unwrap();
    assert!(s1.is_stale);
    assert!(s1.reindex_in_progress);
    let p = s1.progress.expect("progress present");
    assert_eq!(p.phase, "ingest");
    assert_eq!(p.files_seen, 42);
}

/// Task 6.1: two `kenn mcp` instances both reach `Ready` on the same
/// workspace and don't block each other.
#[tokio::test]
async fn two_instances_both_reach_ready_on_same_workspace() {
    let ws = TempDir::new().unwrap();
    let snap = publish_snapshot(ws.path()).await;
    let a = make_state(ws.path());
    let b = make_state(ws.path());
    place_ready(&a, &snap).await;
    place_ready(&b, &snap).await;
    assert_eq!(served_snapshot(&a), snap);
    assert_eq!(served_snapshot(&b), snap);
}

/// Task 6.3: all instances converge on a newer snapshot after any one
/// of them (or a CLI run) publishes it.
#[tokio::test]
async fn instances_converge_on_newest_snapshot() {
    let ws = TempDir::new().unwrap();
    let snap_a = publish_snapshot(ws.path()).await;
    let a = make_state(ws.path());
    let b = make_state(ws.path());
    place_ready(&a, &snap_a).await;
    place_ready(&b, &snap_a).await;

    std::thread::sleep(std::time::Duration::from_millis(1100));
    let snap_b = publish_snapshot(ws.path()).await;

    poll_once(&a, &a.layout(), false, 0).await;
    poll_once(&b, &b.layout(), false, 0).await;
    assert_eq!(served_snapshot(&a), snap_b);
    assert_eq!(served_snapshot(&b), snap_b);
}

/// Task 4.5 (read path): tool dispatch is lock-free w.r.t. an in-flight
/// reindex — `get_index_status` returns even while `Ready.reindex` is
/// `Some`. The architectural guarantee is that `ArcSwap::load_full` and
/// the brief outer read lock are independent of reindex bookkeeping;
/// this test exercises the actual call path.
#[tokio::test]
async fn reads_continue_during_in_flight_reindex() {
    let ws = TempDir::new().unwrap();
    let snap = publish_snapshot(ws.path()).await;
    let state = make_state(ws.path());
    place_ready(&state, &snap).await;

    // Install an in-flight reindex with some progress.
    {
        let mut g = state.lifecycle.write().unwrap();
        if let LifecycleState::Ready { reindex, .. } = &mut *g {
            *reindex = Some(ReindexProgress {
                started_at: Instant::now(),
                progress: Some(ProgressSnapshot {
                    phase: "ingest",
                    files_seen: 7,
                    symbols_seen: 0,
                    edges_seen: 0,
                }),
            });
        }
    }

    // Read paths still serve, and the status surfaces the in-flight run.
    let status = tools::get_index_status(&state, GetIndexStatusArgs::default())
        .unwrap()
        .item
        .unwrap();
    assert_eq!(status.state, "ready");
    assert!(status.reindex_in_progress);
    assert_eq!(status.progress.unwrap().files_seen, 7);
}

/// Task 2.1(b): after a cross-instance reload the served run's
/// `run_event_seq` snaps to this instance's local counter ("caught up as
/// of now"), so `is_stale` reads false immediately — NOT masked by the
/// publishing instance's incommensurable counter — and a subsequent
/// local event flips `is_stale` back to true. This is the watcher-driven
/// -staleness Finding-1 fix (per-process counters are not comparable
/// across instances).
#[tokio::test]
async fn cross_instance_reload_resets_then_redetects_stale() {
    let ws = TempDir::new().unwrap();
    let snap_a = publish_snapshot(ws.path()).await;
    let state = make_state(ws.path());
    place_ready(&state, &snap_a).await;

    // This instance has observed a couple of local events; another
    // process then publishes a newer run.
    state.bump_event_seq();
    state.bump_event_seq();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let snap_b = publish_snapshot(ws.path()).await;

    // Cross-instance reload (the `live`-watch path).
    poll_once(&state, &state.layout(), false, 0).await;
    assert_eq!(served_snapshot(&state), snap_b);

    let s = tools::get_index_status(&state, GetIndexStatusArgs::default())
        .unwrap()
        .item
        .unwrap();
    assert!(
        !s.is_stale,
        "fresh immediately after a cross-instance reload"
    );

    // A new local event makes it stale again — it does not stay masked.
    state.bump_event_seq();
    let s2 = tools::get_index_status(&state, GetIndexStatusArgs::default())
        .unwrap()
        .item
        .unwrap();
    assert!(s2.is_stale, "a new local event after reload flips is_stale");
}

/// Task 6.2 (MCP-level): a snapshot held by an MCP server's
/// `ReaderBinding` survives a GC sweep run by another instance — the
/// reader registry pin propagates through `kenn_store::lifecycle::gc`.
#[tokio::test]
async fn cross_instance_pin_survives_gc_sweep() {
    let ws = TempDir::new().unwrap();
    let snap_a = publish_snapshot(ws.path()).await;
    let instance_a = make_state(ws.path());
    place_ready(&instance_a, &snap_a).await; // A pins snap_a

    // Publish enough newer snapshots that LRU would normally evict A.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let snap_b = publish_snapshot(ws.path()).await;
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let _snap_c = publish_snapshot(ws.path()).await;

    // B is just the second instance, running a GC sweep with `gc_keep=2`.
    let store_b = kenn_store::Store::open(instance_a.layout()).unwrap();
    let deleted = kenn_store::lifecycle::gc(&store_b, 2).unwrap();
    assert!(
        !deleted.contains(&snap_a),
        "snap_a is pinned by instance A's binding — must not be evicted; deleted: {deleted:?}"
    );
    assert!(snap_a.is_dir(), "snap_a directory should still exist");
    assert!(snap_b.is_dir());
}

/// Task 3.8 (embed coordination): when another process holds the
/// per-snapshot embed lock, `embed_pending` skips with a zero report
/// rather than running a redundant embed pass.
#[tokio::test]
async fn embed_pending_skips_when_lock_held() {
    use fs2::FileExt;
    let ws = TempDir::new().unwrap();
    let snap = publish_snapshot(ws.path()).await;
    let layout = Layout::default_for(ws.path());

    // Sanity: the snapshot the test pins its lock to MUST be the one
    // `embed_pending` resolves to. Today both follow `live`; if
    // `decide_startup_state` ever drifts (e.g. picks a sibling by
    // staleness key), the lock we plant below would land at a path
    // `embed_pending` never inspects, and the test would silently
    // stop exercising the contended-lock case.
    let store = Store::open(layout.clone()).unwrap();
    let resolved = match kenn_store::lifecycle::decide_startup_state(&store, ws.path(), false, 0) {
        kenn_store::lifecycle::StartupDecision::Skip { live } => live,
        kenn_store::lifecycle::StartupDecision::Reindex { reason } => {
            panic!("expected Skip after publish_snapshot, got Reindex({reason})")
        }
    };
    assert_eq!(
        resolved, snap,
        "test's snap diverged from decide_startup_state's resolution — \
         the lock would be planted at a path embed_pending never inspects",
    );

    // Hold the per-snapshot embed lock manually (simulating another
    // process mid-embed of the same snapshot). The lock lives at
    // runs/{id}/embed.lock — co-located with the Lance datasets.
    let lock_path = snap.join("embed.lock");
    let hog = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    FileExt::lock_exclusive(&hog).unwrap();

    // `embed_pending` sees the contended lock → returns zero report.
    let report = kenn_store::embed_pending(
        &layout,
        false,
        0,
        &kenn_config::EmbeddingsConfig::default().model,
        kenn_store::shared_embedder(),
    )
    .await
    .unwrap();
    assert_eq!(report.vectors, 0, "embed should skip when peer holds lock");
}
