//! Integration tests for the in-process file watcher.
//!
//! The watcher uses the real `notify` crate against a real `TempDir`.
//! Debounce timing is real-time (not `tokio::time::pause`) because
//! `notify` runs on a non-tokio OS thread — pausing tokio's clock
//! doesn't pause filesystem-event delivery, and waiting for that delivery
//! requires real wall-clock time anyway. Tests use a short
//! `mcp.watch_debounce_ms` (200 ms) and poll for the trigger counter
//! with a generous timeout. On macOS especially, `FSEvents` latency
//! shapes the test timings.
//!
//! The watcher's trigger expiry calls `spawn_background_reindex` which
//! spawns a task that will fail in these tests (no real indexer
//! toolchain in the temp workspace). The failure is harmless and
//! observed only via tracing; tests assert the `watcher_triggers`
//! counter on `ServerState` which is bumped before that spawn.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kenn_config::Config;
use kenn_mcp::state::WatcherState;
use kenn_mcp::tools::{self, ServerState, WatchStartArgs, WatchStopArgs};
use tempfile::TempDir;

mod common;
use common::{make_state_with_config, place_ready, publish_snapshot};

/// Test debounce: chosen long enough that platform fs-event latency
/// (especially macOS `FSEvents`, which can take several hundred ms to
/// start emitting after watch attachment) fits inside the window,
/// short enough to keep tests fast.
const TEST_DEBOUNCE_MS: u64 = 800;
/// Wall-clock time the watcher gets to "settle" after `watch_start`
/// before the test begins writing files — gives FSEvents/inotify
/// time to attach.
const FS_SETTLE_MS: u64 = 500;
/// Window the test waits for an expected trigger after the last write.
/// Must exceed `TEST_DEBOUNCE_MS` + platform latency.
const TRIGGER_WAIT: Duration = Duration::from_secs(5);

fn default_test_config() -> Config {
    let mut c = Config::default();
    c.mcp.watch_debounce_ms = TEST_DEBOUNCE_MS;
    c
}

/// Wait for a closure to return `true`, polling every 20 ms up to
/// `timeout`. Returns true if the condition became true, false if the
/// timeout expired.
async fn wait_until<F: Fn() -> bool>(timeout: Duration, f: F) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    f()
}

/// Set up a Ready server with the watcher already started; returns the
/// workspace tempdir (keep it alive) and the state.
async fn ready_with_watcher(config: Config) -> (TempDir, Arc<ServerState>) {
    let ws = TempDir::new().expect("tempdir");
    let snap = publish_snapshot(ws.path()).await;
    let state = make_state_with_config(ws.path(), config);
    place_ready(&state, &snap).await;
    let resp = tools::watch_start(&state, WatchStartArgs::default()).expect("watch_start");
    assert!(resp.item.expect("watch_start payload").started);
    // Let the notify watcher attach before the test starts writing.
    tokio::time::sleep(Duration::from_millis(FS_SETTLE_MS)).await;
    (ws, state)
}

// ── tool-level tests (no real fs activity required) ─────────────────────────

/// 6.7: `watch_stop` when no watcher is running succeeds (idempotent no-op).
#[tokio::test]
async fn watch_stop_when_idle_is_noop() {
    let ws = TempDir::new().unwrap();
    let snap = publish_snapshot(ws.path()).await;
    let state = make_state_with_config(ws.path(), default_test_config());
    place_ready(&state, &snap).await;
    let resp = tools::watch_stop(&state, WatchStopArgs::default()).expect("watch_stop ok");
    assert!(!resp.item.expect("watch_stop payload").stopped);
    assert_eq!(state.watcher_state.load(), WatcherState::Off);
}

/// 6.8: `watch_start` while a watcher is running returns
/// `WatchStartResult { started: false, debounce_ms }` (no second
/// watcher created).
#[tokio::test]
async fn watch_start_is_idempotent() {
    let (_ws, state) = ready_with_watcher(default_test_config()).await;
    let resp = tools::watch_start(&state, WatchStartArgs::default()).expect("watch_start 2");
    let payload = resp.item.expect("payload");
    assert!(
        !payload.started,
        "second watch_start should not start a new watcher"
    );
    assert_eq!(payload.debounce_ms, TEST_DEBOUNCE_MS);
}

/// 6.9: `watch_stop` followed by `watch_start` produces a fresh
/// watcher (no implicit auto-restart between calls).
#[tokio::test]
async fn watch_stop_then_start_creates_fresh_watcher() {
    let mut config = default_test_config();
    config.mcp.watch_on = true; // even with watch_on, stop is permanent
    let (_ws, state) = ready_with_watcher(config).await;

    let stop = tools::watch_stop(&state, WatchStopArgs::default()).expect("watch_stop");
    assert!(stop.item.expect("payload").stopped);
    assert_eq!(state.watcher_state.load(), WatcherState::Off);

    // Verify it didn't auto-restart.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(state.watcher_state.load(), WatcherState::Off);

    let start = tools::watch_start(&state, WatchStartArgs::default()).expect("watch_start");
    let payload = start.item.expect("payload");
    assert!(
        payload.started,
        "fresh watch_start after stop must start a new watcher"
    );
}

/// 6.11: `watch_start` against a non-Ready server returns an error
/// naming the current state.
#[tokio::test]
async fn watch_start_against_indexing_state_errors() {
    let ws = TempDir::new().unwrap();
    let state = make_state_with_config(ws.path(), default_test_config());
    // Default state is Indexing — leave it.
    let err = tools::watch_start(&state, WatchStartArgs::default())
        .expect_err("watch_start should error in Indexing");
    assert!(
        err.message.contains("indexing"),
        "error message should name the current state, got {err:?}",
    );
}

// ── notification shape (task 2.2 / 6.12) ────────────────────────────────────

/// 2.2 + 6.12: the snapshot-swap notification payload is `code_updated`
/// with a human-readable message — regardless of who caused the swap.
#[test]
fn code_updated_payload_shape() {
    let v = kenn_mcp::indexing::code_updated_payload("2026-05-23T14-23-05Z");
    assert_eq!(v["event"], "code_updated");
    assert!(v["message"]
        .as_str()
        .unwrap()
        .contains("2026-05-23T14-23-05Z"));
    // Must NOT leak the internal `snapshot_id` field.
    assert!(v.get("snapshot_id").is_none());
}

// ── real-notify integration tests ───────────────────────────────────────────

/// 6.2: 3 saves within the debounce window collapse to exactly 1
/// trigger.
#[tokio::test]
async fn burst_of_saves_collapses_to_one_trigger() {
    let (ws, state) = ready_with_watcher(default_test_config()).await;

    // 3 quick writes well inside the debounce window.
    for i in 0..3 {
        std::fs::write(ws.path().join(format!("file{i}.rs")), "fn x() {}\n").unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let got = wait_until(TRIGGER_WAIT, || {
        state.watcher_triggers.load(Ordering::Relaxed) >= 1
    })
    .await;
    assert!(
        got,
        "no trigger observed (triggers={})",
        state.watcher_triggers.load(Ordering::Relaxed)
    );

    // Settle: any in-flight pings drain. No second trigger should fire
    // because all 3 writes landed in the same window.
    tokio::time::sleep(Duration::from_millis(TEST_DEBOUNCE_MS + 400)).await;
    assert_eq!(
        state.watcher_triggers.load(Ordering::Relaxed),
        1,
        "burst should produce exactly 1 trigger"
    );
}

/// 6.3: writes under a `WORKSPACE_SKIP_DIRS` path do not trigger.
#[tokio::test]
async fn writes_under_skip_dir_do_not_trigger() {
    let (ws, state) = ready_with_watcher(default_test_config()).await;
    std::fs::create_dir_all(ws.path().join("target/debug")).unwrap();
    std::fs::write(ws.path().join("target/debug/x.rs"), "fn x() {}\n").unwrap();

    // Wait beyond debounce + platform latency; trigger count must stay 0.
    tokio::time::sleep(Duration::from_millis(TEST_DEBOUNCE_MS + 800)).await;
    assert_eq!(state.watcher_triggers.load(Ordering::Relaxed), 0);
}

/// 6.4: writes under a user-configured `[exclude] globs` entry do not
/// trigger.
#[tokio::test]
async fn writes_under_user_exclude_glob_do_not_trigger() {
    let mut config = default_test_config();
    config.workspace.excludes = vec!["**/generated/**".into()];
    let (ws, state) = ready_with_watcher(config).await;
    std::fs::create_dir_all(ws.path().join("src/generated")).unwrap();
    std::fs::write(ws.path().join("src/generated/Foo.cs"), "class X {}\n").unwrap();

    tokio::time::sleep(Duration::from_millis(TEST_DEBOUNCE_MS + 800)).await;
    assert_eq!(state.watcher_triggers.load(Ordering::Relaxed), 0);
}

/// 6.5: a deletion of a tracked source file contributes to the debounce
/// window (same treatment as a save).
#[tokio::test]
async fn deletion_of_source_file_triggers_debounce() {
    let (ws, state) = ready_with_watcher(default_test_config()).await;
    // Create the file *before* starting the burst window so we can
    // delete it cleanly. Wait for the creation event to flush and the
    // debounce that ensues to expire — we want to start the deletion
    // test from a known counter baseline.
    std::fs::write(ws.path().join("doomed.rs"), "fn x() {}\n").unwrap();
    let _ = wait_until(TRIGGER_WAIT, || {
        state.watcher_triggers.load(Ordering::Relaxed) >= 1
    })
    .await;
    // Settle.
    tokio::time::sleep(Duration::from_millis(TEST_DEBOUNCE_MS + 400)).await;
    let baseline = state.watcher_triggers.load(Ordering::Relaxed);

    std::fs::remove_file(ws.path().join("doomed.rs")).unwrap();
    let got = wait_until(TRIGGER_WAIT, || {
        state.watcher_triggers.load(Ordering::Relaxed) > baseline
    })
    .await;
    assert!(got, "deletion did not produce a trigger");
}

/// 6.6: `watch_stop` mid-debounce cancels the pending trigger.
#[tokio::test]
async fn watch_stop_mid_debounce_cancels_trigger() {
    let (ws, state) = ready_with_watcher(default_test_config()).await;
    let baseline = state.watcher_triggers.load(Ordering::Relaxed);

    // Land an event to enter Debouncing.
    std::fs::write(ws.path().join("a.rs"), "fn a() {}\n").unwrap();
    let entered = wait_until(TRIGGER_WAIT, || {
        state.watcher_state.load() == WatcherState::Debouncing
    })
    .await;
    assert!(
        entered,
        "watcher did not enter Debouncing (state={:?})",
        state.watcher_state.load()
    );

    // Stop before the window expires.
    let stop = tools::watch_stop(&state, WatchStopArgs::default()).expect("watch_stop");
    assert!(stop.item.expect("payload").stopped);

    // Wait well beyond the window; trigger counter must NOT advance.
    tokio::time::sleep(Duration::from_millis(TEST_DEBOUNCE_MS + 600)).await;
    assert_eq!(
        state.watcher_triggers.load(Ordering::Relaxed),
        baseline,
        "watch_stop must cancel the pending trigger"
    );
    assert_eq!(state.watcher_state.load(), WatcherState::Off);
}

// ── boot path (task 6.10) ───────────────────────────────────────────────────

/// 6.10: `mcp.watch_on = true` boots the server with the watcher
/// active. Calls `autostart_watcher` directly — the same code path
/// invoked by `start_background_indexing` after the lifecycle reaches
/// `Ready`. We can't drive the full `start_background_indexing` here
/// because that requires running the real indexer pipeline; the boot
/// wiring's `watch_on` check and Ready-wait are simple branches,
/// verified by inspection. `autostart_watcher` is the actual install
/// path and is what this test exercises.
#[tokio::test]
async fn watch_on_boots_server_with_watcher() {
    let mut config = default_test_config();
    config.mcp.watch_on = true;
    let ws = TempDir::new().expect("tempdir");
    let snap = publish_snapshot(ws.path()).await;
    let state = make_state_with_config(ws.path(), config);
    place_ready(&state, &snap).await;

    // No `watch_start` call. This is what the boot path does:
    kenn_mcp::indexing::autostart_watcher(&state).expect("autostart");

    // Watcher should now be installed and Idle.
    assert!(
        state.watcher.lock().expect("watcher mutex").is_some(),
        "autostart_watcher must install a handle"
    );
    tokio::time::sleep(Duration::from_millis(FS_SETTLE_MS)).await;
    assert_eq!(state.watcher_state.load(), WatcherState::Idle);
}
