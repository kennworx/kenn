//! meta.json `backend` marker mismatch detection.
//!
//! Build a fake snapshot with a `backend` value that disagrees with the
//! active backend; confirm `kenn_store::open_reader` fails clean.

use kenn_store::{open_reader, ACTIVE_BACKEND};
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread")]
async fn open_reader_rejects_wrong_backend_marker() {
    // A snapshot built by the legacy backend carries a different marker.
    let other = "surreal";

    let dir = TempDir::new().unwrap();
    // Write a meta.json that claims the other backend.
    let meta = serde_json::json!({
        "timestamp": "2026-05-07T00:00:00Z",
        "status": "success",
        "backend": other,
        "documents": 0,
        "symbols": 0,
        "definitions": 0,
        "edges": 0,
    });
    std::fs::write(
        dir.path().join("meta.json"),
        serde_json::to_vec_pretty(&meta).unwrap(),
    )
    .unwrap();

    let result = open_reader(dir.path()).await;
    let Err(err) = result else {
        panic!("open_reader must fail on backend mismatch");
    };
    let msg = format!("{err}");
    assert!(
        msg.contains(other),
        "error message should name the snapshot's backend ({other}): {msg}"
    );
    assert!(
        msg.contains(ACTIVE_BACKEND),
        "error message should name the active backend ({ACTIVE_BACKEND}): {msg}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn open_reader_tolerates_missing_marker_in_legacy_meta() {
    // A meta.json without the `backend` field should NOT trip the check
    // (legacy snapshots from before this change must still open).
    let dir = TempDir::new().unwrap();
    let meta = serde_json::json!({
        "timestamp": "2026-05-07T00:00:00Z",
        "status": "success",
        "documents": 0,
    });
    std::fs::write(
        dir.path().join("meta.json"),
        serde_json::to_vec_pretty(&meta).unwrap(),
    )
    .unwrap();

    // We expect open_reader to fail downstream (no actual db), but
    // NOT with the backend-mismatch error.
    let result = open_reader(dir.path()).await;
    if let Err(e) = result {
        let msg = format!("{e}");
        assert!(
            !msg.contains("snapshot was built by"),
            "missing marker should not trigger mismatch: {msg}"
        );
    }
}
