//! Lifecycle-related integration tests for the mcp-owned-indexing
//! foundation: tools fail-fast while not Ready, status tool always
//! works, and the lifecycle transitions match the spec scenarios.

use kenn_mcp::error::McpErrorCode;
use kenn_mcp::tools::{
    get_index_status, get_workspace_overview, search_symbols, GetIndexStatusArgs,
    GetWorkspaceOverviewArgs, SearchSymbolsArgs, ServerState,
};
use tempfile::TempDir;

mod common;
use common::{make_state, place_ready, publish_snapshot};

#[tokio::test(flavor = "multi_thread")]
async fn no_live_snapshot_reports_indexing_and_blocks_other_tools() {
    let dir = TempDir::new().unwrap();
    let _ = kenn_store::Store::open_default(dir.path()).unwrap();
    let state = ServerState::new(dir.path());

    // Status tool: succeeds with state=indexing.
    let resp = get_index_status(&state, GetIndexStatusArgs::default()).unwrap();
    assert!(resp.found);
    let s = resp.item.unwrap();
    assert_eq!(s.state, "indexing");
    assert!(s.snapshot_id.is_none());
    assert!(s.error.is_none());

    let err = get_workspace_overview(&state, GetWorkspaceOverviewArgs::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, McpErrorCode::IndexUnavailable);
    assert!(err.message.to_lowercase().contains("indexing"));

    let err = search_symbols(
        &state,
        &SearchSymbolsArgs {
            query: "anything".into(),
            filters: None,
            pagination: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, McpErrorCode::IndexUnavailable);
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_indexing_is_terminal_and_reports_via_status() {
    use kenn_config::Config;
    use kenn_mcp::state::LifecycleState;

    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("kenn.toml"),
        "[language.csharp]\n\
         enabled = true\n\
         projects = [\"does-not-exist.sln\"]\n\
         command = [\"/usr/bin/false\"]\n",
    )
    .unwrap();

    let state = ServerState::new(dir.path());
    let config = Config::load_from_path(&dir.path().join("kenn.toml")).unwrap();

    let result = kenn_indexer::index_workspace(
        &kenn_store::Layout::default_for(dir.path()),
        &config,
        |_ev| {},
        kenn_indexer::pipeline::no_op_hook(),
    )
    .await;
    {
        let mut g = state.lifecycle.write().unwrap();
        match result {
            Ok(_) => panic!("expected workflow to fail"),
            Err(e) => {
                *g = LifecycleState::Failed {
                    error: e.to_string(),
                    started_at: std::time::Instant::now(),
                    ended_at: std::time::Instant::now(),
                };
            }
        }
    }

    let resp = get_index_status(&state, GetIndexStatusArgs::default()).unwrap();
    let s = resp.item.unwrap();
    assert_eq!(s.state, "failed");
    assert!(s.error.is_some());
    assert!(!s.error.unwrap().is_empty());

    let err = get_workspace_overview(&state, GetWorkspaceOverviewArgs::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, McpErrorCode::IndexUnavailable);
}

#[tokio::test(flavor = "multi_thread")]
async fn live_snapshot_bootstraps_to_ready() {
    use assert_cmd::Command;

    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("kenn.toml"),
        "[language.csharp]\nenabled = false\n",
    )
    .unwrap();
    Command::cargo_bin("kenn")
        .unwrap()
        .arg("--workspace")
        .arg(dir.path())
        .arg("init")
        .assert()
        .success();
    Command::cargo_bin("kenn")
        .unwrap()
        .arg("--workspace")
        .arg(dir.path())
        .arg("index")
        .assert()
        .success();

    let state = ServerState::new(dir.path());
    state.bootstrap().await;

    let resp = get_index_status(&state, GetIndexStatusArgs::default()).unwrap();
    let s = resp.item.unwrap();
    assert_eq!(s.state, "ready");
    assert!(s.snapshot_id.is_some());
    assert!(s.error.is_none());

    let overview = get_workspace_overview(&state, GetWorkspaceOverviewArgs::default())
        .await
        .unwrap();
    assert!(overview.found);

    // The embed stage drives the reported `state` once the graph is Ready, and
    // structural tools keep serving across embed stages (mcp-embedding-stage).
    state
        .embed_stage
        .store(kenn_mcp::state::EmbedStage::Building);
    let s = get_index_status(&state, GetIndexStatusArgs::default())
        .unwrap()
        .item
        .unwrap();
    assert_eq!(s.state, "embedding");
    assert!(
        s.snapshot_id.is_some(),
        "ready-shape fields populated while embedding"
    );
    // Structural query works while embedding (does not wait for `ready`).
    assert!(
        get_workspace_overview(&state, GetWorkspaceOverviewArgs::default())
            .await
            .unwrap()
            .found
    );

    state
        .embed_stage
        .store(kenn_mcp::state::EmbedStage::Disabled);
    let s = get_index_status(&state, GetIndexStatusArgs::default())
        .unwrap()
        .item
        .unwrap();
    assert_eq!(s.state, "disabled");
}

/// `get_index_status` surfaces a served snapshot's degraded run
/// (mcp-index-status-degradation): a `partial` meta yields `run_status` +
/// failed-project attributions with true counts, while a clean `success`
/// meta omits the whole block.
#[tokio::test(flavor = "multi_thread")]
async fn index_status_reports_partial_run_and_omits_clean() {
    // Clean snapshot: a genuine success meta (with the required
    // `timestamp`) so this proves parse-then-omit, not a parse failure.
    let clean_ws = TempDir::new().unwrap();
    let clean_snap = publish_snapshot(clean_ws.path()).await;
    std::fs::write(
        clean_snap.join(kenn_indexer::SNAPSHOT_META_FILE),
        serde_json::to_vec(&serde_json::json!({
            "timestamp": "2026-07-07T00-00-00Z",
            "status": "success",
            "schema_version": kenn_store::STORE_SCHEMA_VERSION,
            "documents": 1, "symbols": 1, "definitions": 0, "edges": 0,
        }))
        .unwrap(),
    )
    .unwrap();
    let clean_state = make_state(clean_ws.path());
    place_ready(&clean_state, &clean_snap).await;
    let s = get_index_status(&clean_state, GetIndexStatusArgs::default())
        .unwrap()
        .item
        .unwrap();
    assert!(s.run_status.is_none(), "clean run omits run_status");
    assert!(s.failed_projects.is_empty() && s.warnings.is_empty());
    assert_eq!(
        (s.failed_count, s.warning_count, s.regression_count),
        (0, 0, 0)
    );

    // Partial snapshot: rewrite the published meta.json before binding.
    // Includes overflow (to prove the raw array carries no `+N more` marker)
    // and a metric regression (to prove regressions count as degradation).
    let ws = TempDir::new().unwrap();
    let snap = publish_snapshot(ws.path()).await;
    std::fs::write(
        snap.join(kenn_indexer::SNAPSHOT_META_FILE),
        serde_json::to_vec(&serde_json::json!({
            "timestamp": "2026-07-07T00-00-00Z",
            "status": "partial",
            "schema_version": kenn_store::STORE_SCHEMA_VERSION,
            "documents": 1, "symbols": 1, "definitions": 0, "edges": 0,
            "failed_projects": ["csharp: msbuild failed"],
            "failed_overflow": 4,
            "warnings": ["swift: 2 stale index-store units kept"],
            "regression_warnings": [
                {"metric": "symbols", "previous": 900, "current": 600, "drop_pct": 33}
            ],
        }))
        .unwrap(),
    )
    .unwrap();
    let state = make_state(ws.path());
    place_ready(&state, &snap).await;
    let s = get_index_status(&state, GetIndexStatusArgs::default())
        .unwrap()
        .item
        .unwrap();
    assert_eq!(s.run_status.as_deref(), Some("partial"));
    assert_eq!(s.failed_count, 5, "1 listed + 4 overflow");
    // Raw structured array — one real entry, no `+N more` display marker.
    assert_eq!(s.failed_projects.len(), 1);
    assert!(s.failed_projects.iter().any(|p| p.contains("csharp")));
    assert!(!s.failed_projects.iter().any(|p| p.contains("more")));
    assert_eq!(s.warning_count, 1);
    assert_eq!(s.regression_count, 1);
    // Degradation is reported, not escalated — the graph still serves.
    assert!(s.snapshot_id.is_some());
}
