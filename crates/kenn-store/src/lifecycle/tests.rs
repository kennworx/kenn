use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::layout::Store;

use super::atomic::atomic_flip_live;
use super::indexing::begin_indexing;
use super::ops::{gc, rollback};
use super::recover::recover;
use super::state::{current_state, decide_startup_state, list_completed_runs};
use super::types::{
    BeginError, LifecycleState, PublishError, RollbackError, StartupDecision, META_FILE,
};

fn store() -> (TempDir, Store) {
    let ws = TempDir::new().unwrap();
    let s = Store::open_default(ws.path()).unwrap();
    (ws, s)
}

/// Write the run's completion-stamp `meta.json` with a chosen
/// staleness key. Mirrors what the pipeline does after a
/// successful pass.
fn write_meta_with_staleness(run_dir: &Path, key: &crate::staleness::StalenessKey) {
    let meta = serde_json::json!({
        "timestamp": "x", "status": "success",
        "schema_version": crate::STORE_SCHEMA_VERSION,
        "backend": crate::ACTIVE_BACKEND,
        "documents": 0, "symbols": 0, "definitions": 0, "edges": 0,
        "staleness_key": key,
    });
    fs::write(run_dir.join(META_FILE), serde_json::to_vec(&meta).unwrap()).unwrap();
}

/// Write a minimal `meta.json` with no staleness key — sufficient
/// to mark the run as "published" for tests that don't exercise
/// the staleness path.
fn mark_complete(run_dir: &Path) {
    fs::write(run_dir.join(META_FILE), b"{\"status\":\"success\"}").unwrap();
}

#[test]
fn current_state_uninitialized() {
    let (_ws, s) = store();
    assert_eq!(current_state(&s), LifecycleState::Uninitialized);
}

#[test]
fn begin_then_publish_flips_live() {
    let (_ws, s) = store();
    let h = begin_indexing(&s).unwrap();
    // Pipeline writes data + the completion stamp.
    std::fs::write(h.run_dir().join("data"), b"hello").unwrap();
    mark_complete(h.run_dir());
    let run = h.publish().unwrap();
    assert!(run.is_dir());
    assert_eq!(std::fs::read(run.join("data")).unwrap(), b"hello");
    let live = s.live_target().unwrap();
    assert_eq!(live, run);
    match current_state(&s) {
        LifecycleState::Steady { live: l } => assert_eq!(l, run),
        other => panic!("expected Steady, got {other:?}"),
    }
}

#[test]
fn publish_without_meta_errors() {
    let (_ws, s) = store();
    let h = begin_indexing(&s).unwrap();
    // No meta.json written → publish refuses.
    match h.publish() {
        Err(PublishError::NoMeta) => {}
        other => panic!("expected NoMeta, got {other:?}"),
    }
}

#[test]
fn abort_leaves_live_unchanged() {
    let (_ws, s) = store();
    // Pre-populate a previous live so we can verify it's untouched.
    let h = begin_indexing(&s).unwrap();
    mark_complete(h.run_dir());
    let prev = h.publish().unwrap();

    let h2 = begin_indexing(&s).unwrap();
    let aborted = h2.run_dir().to_path_buf();
    h2.abort().unwrap();
    assert!(!aborted.exists());
    assert_eq!(s.live_target().unwrap(), prev);
}

#[test]
fn second_begin_blocked_by_lock() {
    let (_ws, s) = store();
    let _h = begin_indexing(&s).unwrap();
    match begin_indexing(&s) {
        Err(BeginError::LockHeld(_)) => {}
        other => panic!("expected LockHeld, got {other:?}"),
    }
}

#[test]
fn dropped_handle_cleans_incomplete_run() {
    let (_ws, s) = store();
    let dir;
    {
        let h = begin_indexing(&s).unwrap();
        dir = h.run_dir().to_path_buf();
        assert!(dir.exists());
    }
    // After drop, the incomplete run is gone and the lock is
    // released.
    assert!(!dir.exists());
    let _h2 = begin_indexing(&s).unwrap();
}

#[test]
fn dropped_handle_retains_run_with_meta() {
    // Regression: a handle dropped after the pipeline wrote
    // meta.json (the completion stamp) but before `publish()`
    // succeeded — e.g. a publish that failed at fsync_dir or
    // atomic_flip_live, or a panic between meta.json write and
    // publish call — must retain the run. D1 invariant: meta.json
    // present ⇒ run is complete and eligible for `recover` /
    // `gc` / `rollback` to consider.
    let (_ws, s) = store();
    let dir;
    {
        let h = begin_indexing(&s).unwrap();
        dir = h.run_dir().to_path_buf();
        // Pipeline writes the completion stamp.
        mark_complete(h.run_dir());
        // Handle is dropped without `publish()` (simulating the
        // failure window after meta.json write).
    }
    // The complete run survives the Drop — `recover()` on next
    // start treats it as a retained run, not as orphan-to-clean.
    assert!(dir.exists(), "Drop must retain a run carrying meta.json");
    assert!(dir.join(META_FILE).is_file());
    // `recover()` also leaves it alone (meta.json is the gate).
    let r = recover(&s).unwrap();
    assert!(r.deleted_incomplete_runs.is_empty());
    assert!(dir.exists());
}

#[test]
fn recover_deletes_incomplete_runs() {
    let (_ws, s) = store();
    let incomplete = s.runs_dir().join("2026-05-01T00-00-00Z");
    fs::create_dir_all(&incomplete).unwrap();
    std::fs::write(incomplete.join("garbage"), b"x").unwrap();
    // A second run, but completed.
    let complete = s.runs_dir().join("2026-05-02T00-00-00Z");
    fs::create_dir_all(&complete).unwrap();
    mark_complete(&complete);

    let r = recover(&s).unwrap();
    assert_eq!(r.deleted_incomplete_runs.len(), 1);
    assert!(!incomplete.exists());
    assert!(complete.exists(), "the completed run survives recovery");
}

#[test]
fn rollback_with_no_previous_errors() {
    let (_ws, s) = store();
    let h = begin_indexing(&s).unwrap();
    mark_complete(h.run_dir());
    h.publish().unwrap();
    match rollback(&s) {
        Err(RollbackError::NoPrevious) => {}
        other => panic!("expected NoPrevious, got {other:?}"),
    }
}

#[test]
fn rollback_walks_back_one_run() {
    let (_ws, s) = store();
    // Run A
    let h = begin_indexing(&s).unwrap();
    mark_complete(h.run_dir());
    let a = h.publish().unwrap();
    // Wait so the second run gets a later second.
    std::thread::sleep(std::time::Duration::from_secs(1));
    // Run B (now live)
    let h2 = begin_indexing(&s).unwrap();
    mark_complete(h2.run_dir());
    let b = h2.publish().unwrap();
    assert_eq!(s.live_target().unwrap(), b);

    let target = rollback(&s).unwrap();
    assert_eq!(target, a);
    assert_eq!(s.live_target().unwrap(), a);
}

#[test]
fn gc_keeps_n_and_drops_the_rest() {
    let (_ws, s) = store();
    // Create three completed runs with distinct ids.
    for ts in [
        "2026-05-01T00-00-00Z",
        "2026-05-02T00-00-00Z",
        "2026-05-03T00-00-00Z",
    ] {
        let dir = s.runs_dir().join(ts);
        fs::create_dir_all(&dir).unwrap();
        mark_complete(&dir);
    }
    // Point live at the newest.
    fs::write(s.live_path(), "runs/2026-05-03T00-00-00Z").unwrap();

    // Retain 2 → keep the 2 most-recently-accessed (which tiebreaks
    // on id), drop the third. Live is exempt from eviction in
    // addition to rank-based retention.
    let deleted = gc(&s, 2).unwrap();
    assert_eq!(
        deleted.len(),
        1,
        "exactly one (the oldest non-live) evicted"
    );
    let kept: Vec<_> = list_completed_runs(&s).into_iter().map(|m| m.id).collect();
    assert!(
        kept.contains(&"2026-05-03T00-00-00Z".to_string()),
        "live retained"
    );
    assert!(
        !kept.contains(&"2026-05-01T00-00-00Z".to_string()),
        "oldest evicted"
    );
}

#[test]
fn decide_startup_state_matches_recorded_staleness_key() {
    let (ws, s) = store();
    // Create a run with a deliberate Unknown key so it never matches
    // a "current" staleness probe.
    let other = s.runs_dir().join("2026-05-01T00-00-00Z");
    fs::create_dir_all(&other).unwrap();
    write_meta_with_staleness(&other, &crate::staleness::StalenessKey::Unknown);

    let dec = decide_startup_state(&s, ws.path(), true, 0);
    // Unknown vs whatever the current tree fingerprint computes —
    // they don't match, so this is Reindex.
    assert!(
        matches!(dec, StartupDecision::Reindex { .. }),
        "got {dec:?}"
    );
}

/// The config-blind-staleness fix: a run published under one
/// `config_sig` is `Skip`-served only when the startup decision uses the
/// SAME `config_sig`. A different `config_sig` (a changed `[language.*]`
/// config, unchanged workspace) yields a non-matching key → `Reindex`.
#[test]
fn decide_startup_state_reindexes_when_config_sig_changes() {
    let (ws, s) = store();
    // Publish a run stamped with the workspace key under config A.
    let sig_a = 0xA11C_E101_u64;
    let run = s.runs_dir().join("2026-05-01T00-00-00Z");
    fs::create_dir_all(&run).unwrap();
    let key_a = crate::staleness::compute_staleness_key(ws.path(), sig_a);
    write_meta_with_staleness(&run, &key_a);

    // Same config_sig → the recorded key matches → Skip.
    assert!(
        matches!(
            decide_startup_state(&s, ws.path(), true, sig_a),
            StartupDecision::Skip { .. }
        ),
        "same config_sig + unchanged workspace must Skip"
    );

    // A changed language config (different sig), same workspace → the
    // recorded key no longer matches → Reindex.
    let sig_b = 0xB0B0_2222_u64;
    assert!(
        matches!(
            decide_startup_state(&s, ws.path(), true, sig_b),
            StartupDecision::Reindex { .. }
        ),
        "a changed config_sig must force Reindex even on an unchanged workspace"
    );
}

/// A run whose staleness key matches the workspace but whose recorded
/// `schema_version` is stale is **not** servable: `decide_startup_state`
/// must `Reindex` rather than `Skip`. This is the schema-bump case — the
/// source is unchanged, so a staleness-only check would loop forever
/// serving a snapshot that fails to open.
#[test]
fn decide_startup_state_reindexes_on_stale_schema_despite_key_match() {
    let (ws, s) = store();
    let current = crate::staleness::compute_staleness_key(ws.path(), 0);
    let run = s.runs_dir().join("2026-05-01T00-00-00Z");
    fs::create_dir_all(&run).unwrap();
    // Matching key, but a schema version one behind this binary.
    let meta = serde_json::json!({
        "timestamp": "x", "status": "success",
        "schema_version": crate::STORE_SCHEMA_VERSION - 1,
        "backend": crate::ACTIVE_BACKEND,
        "staleness_key": current,
    });
    fs::write(run.join(META_FILE), serde_json::to_vec(&meta).unwrap()).unwrap();

    let dec = decide_startup_state(&s, ws.path(), true, 0);
    assert!(
        matches!(dec, StartupDecision::Reindex { .. }),
        "stale schema must force Reindex even on a key match; got {dec:?}"
    );
}

/// Repoint `live` from `runs/A` to `runs/B` at index-pass cadence
/// (one flip per 50 ms — far faster than any real `kenn index` pass,
/// which is minutes apart) while a reader resolves the pointer at the
/// same cadence (one `read_to_string` per 100 ms — far faster than the
/// indexer, which resolves `live` once per startup). Every observed
/// pointer MUST be one of the two valid relative paths, and no read
/// MUST fail OR observe an absent, empty, or truncated pointer. This
/// locks in the write-temp-then-`rename` pattern (D1) against realistic
/// readers.
///
/// Empty/truncated is the failure a symlink could not produce and a
/// pointer file could: `atomic_flip_live` writes a complete temp file
/// and renames it over `live`, so a reader sees the whole old file or
/// the whole new one, never a partial write. Mutation-checked by
/// writing the pointer in place instead of via rename — that turns this
/// test red (task 4.2).
#[test]
fn live_pointer_repoint_is_atomic_for_realistic_readers() {
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    const READER_GAP: Duration = Duration::from_millis(100);
    const WRITER_GAP: Duration = Duration::from_millis(50);
    const WRITER_ITERS: usize = 60;

    let (_ws, s) = store();
    let run_a = s.runs_dir().join("2026-05-01T00-00-00Z");
    let run_b = s.runs_dir().join("2026-05-02T00-00-00Z");
    fs::create_dir_all(&run_a).unwrap();
    fs::create_dir_all(&run_b).unwrap();

    // Bootstrap `live` → A so the reader sees a valid pointer from its
    // very first read.
    atomic_flip_live(&s.live_path(), &run_a).unwrap();

    // Stored relative to `derived_root` (live's own parent).
    let expected_a = "runs/2026-05-01T00-00-00Z";
    let expected_b = "runs/2026-05-02T00-00-00Z";

    let stop = Arc::new(AtomicBool::new(false));
    let read_errors = Arc::new(AtomicUsize::new(0));
    let reads = Arc::new(AtomicUsize::new(0));
    let first_error: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));

    let live_path = s.live_path();
    let stop_r = Arc::clone(&stop);
    let errs_r = Arc::clone(&read_errors);
    let reads_r = Arc::clone(&reads);
    let report_err = Arc::clone(&first_error);
    let reader = thread::spawn(move || {
        let mut observed: HashSet<String> = HashSet::new();
        while !stop_r.load(Ordering::Relaxed) {
            match fs::read_to_string(&live_path) {
                Ok(raw) => {
                    let t = raw.trim();
                    if t.is_empty() {
                        // Absent-content read: an empty or truncated pointer,
                        // the failure a symlink could not produce.
                        if errs_r.fetch_add(1, Ordering::Relaxed) == 0 {
                            *report_err.lock().unwrap() =
                                Some("empty/truncated pointer".to_string());
                        }
                    } else {
                        observed.insert(t.to_string());
                        reads_r.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(e) => {
                    if errs_r.fetch_add(1, Ordering::Relaxed) == 0 {
                        *report_err.lock().unwrap() = Some(format!("{:?}: {e}", e.kind()));
                    }
                }
            }
            thread::sleep(READER_GAP);
        }
        observed
    });

    let live_w = s.live_path();
    let writer = thread::spawn(move || {
        for i in 0..WRITER_ITERS {
            let target = if i % 2 == 0 { &run_b } else { &run_a };
            atomic_flip_live(&live_w, target).unwrap();
            thread::sleep(WRITER_GAP);
        }
    });

    writer.join().unwrap();
    stop.store(true, Ordering::Relaxed);
    let observed = reader.join().unwrap();

    let err_count = read_errors.load(Ordering::Relaxed);
    let first_err = first_error.lock().unwrap().clone();
    assert_eq!(
        err_count, 0,
        "reader observed {err_count} failed/empty pointer read(s) at 100ms cadence; first error: {first_err:?}",
    );
    assert!(
        reads.load(Ordering::Relaxed) > 0,
        "reader thread never ran a successful pointer read",
    );
    for t in &observed {
        assert!(
            t == expected_a || t == expected_b,
            "observed unexpected pointer {t:?}; expected {expected_a:?} or {expected_b:?}",
        );
    }
}
