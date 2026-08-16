//! The gate, and the lifecycle tools that read it.
//!
//! The helper tests that used to live here (`split_public_id`, `parse_kind`,
//! `slice_lines`) moved to `kenn-query` with the helpers themselves.

use super::{get_index_status, wait_for_index, GetIndexStatusArgs, ServerState, WaitForIndexArgs};

use kenn_store::Store;

use kenn_query::QueryErrorCode;

use tempfile::TempDir;

#[test]
fn status_reports_indexing_when_no_live() {
    let dir = TempDir::new().unwrap();
    let _ = Store::open_default(dir.path()).unwrap();
    let state = ServerState::new(dir.path());
    let resp = get_index_status(&state, GetIndexStatusArgs::default()).unwrap();
    assert!(resp.found);
    let s = resp.item.unwrap();
    assert_eq!(s.state, "indexing");
    assert!(s.snapshot_id.is_none());
}

/// Both gates refuse a not-yet-`Ready` server, and both say
/// `INDEX_UNAVAILABLE` rather than anything about the snapshot.
///
/// This replaces four tests that named four different tools
/// (`get_symbol_rejects_empty_id`, …) but, once the gate moved onto
/// `open_query`, all ran the identical two lines and called no tool at all —
/// four names for one assertion. The argument validation those names described
/// is now covered where it happens, against a `Ready` server, in
/// `tests/symbol_search.rs`.
///
/// Ordering matters here: a caller who cannot be served at all must not first
/// be told something about a snapshot it was never going to read.
#[tokio::test(flavor = "multi_thread")]
async fn both_gates_report_index_unavailable_when_no_live() {
    let dir = TempDir::new().unwrap();
    let _ = Store::open_default(dir.path()).unwrap();
    let state = ServerState::new(dir.path());
    let err = state.open_query_allow_empty().err().expect("not ready");
    assert_eq!(err.code, QueryErrorCode::IndexUnavailable);
    let err = state.open_query().await.err().expect("not ready");
    assert_eq!(err.code, QueryErrorCode::IndexUnavailable);
}

#[tokio::test(flavor = "multi_thread")]
async fn wait_for_index_times_out_while_indexing() {
    let dir = TempDir::new().unwrap();
    // `ServerState::new` starts in the `Indexing` state (never settles
    // here — no pipeline is driven), so the wait must time out.
    let state = ServerState::new(dir.path());
    let start = std::time::Instant::now();
    let resp = wait_for_index(
        &state,
        WaitForIndexArgs {
            timeout_ms: Some(120),
        },
    )
    .await
    .unwrap();
    let elapsed = start.elapsed();
    let item = resp.item.unwrap();
    assert!(item.timed_out, "should time out while indexing");
    assert_eq!(item.status.state, "indexing");
    assert!(
        elapsed >= std::time::Duration::from_millis(120),
        "waited only {elapsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn wait_for_index_returns_immediately_on_failed() {
    let dir = TempDir::new().unwrap();
    let state = ServerState::new(dir.path());
    {
        let mut g = state.lifecycle.write().unwrap();
        *g = crate::state::LifecycleState::Failed {
            error: "boom".into(),
            started_at: std::time::Instant::now(),
            ended_at: std::time::Instant::now(),
        };
    }
    let start = std::time::Instant::now();
    let resp = wait_for_index(&state, WaitForIndexArgs::default())
        .await
        .unwrap();
    let item = resp.item.unwrap();
    assert!(!item.timed_out, "failed is a settled state");
    assert_eq!(item.status.state, "failed");
    assert!(
        start.elapsed() < std::time::Duration::from_millis(500),
        "should return promptly, not wait the full default timeout"
    );
}
