//! End-to-end test: publish a snapshot via the CLI, then drive every
//! tool through `ServerState` against the published live snapshot.
//!
//! The CLI runs in a child process so `RocksDB` releases its lock before
//! the test process opens a read-only `Reader`.

use std::path::Path;

use assert_cmd::Command;
use kenn_mcp::tools::{
    get_index_status, wait_for_index, GetIndexStatusArgs, ServerState, WaitForIndexArgs,
};
use kenn_query::QueryErrorCode;
use kenn_query::{
    find_predecessors, get_workspace_overview, merge_findings, store_finding, FindingDagArgs,
    GetWorkspaceOverviewArgs, MergeFindingsArgs, StoreFindingArgs,
};
use tempfile::TempDir;

fn run_cli(workspace: &Path, args: &[&str]) {
    Command::cargo_bin("kenn")
        .expect("locate kenn binary")
        .arg("--workspace")
        .arg(workspace)
        .args(args)
        .assert()
        .success();
}

#[tokio::test(flavor = "multi_thread")]
async fn published_empty_workspace_serves_status_and_overview() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("kenn.toml"),
        "[language.csharp]\nenabled = false\n",
    )
    .unwrap();
    run_cli(dir.path(), &["init"]);
    run_cli(dir.path(), &["index"]);

    let state = ServerState::new(dir.path());
    state.bootstrap().await;

    let status = get_index_status(&state, GetIndexStatusArgs::default()).unwrap();
    assert!(status.found);
    let s = status.item.unwrap();
    assert_eq!(s.state, "ready");
    let snapshot_id = s.snapshot_id.expect("snapshot_id present in Ready");
    assert_eq!(snapshot_id.len(), 12, "snapshot_id should be 12 hex chars");
    assert!(!s.is_stale);
    assert!(!s.fallback_from_parent_worktree);

    let view = state.open_query_allow_empty().expect("ready");
    let ctx = state.query_ctx(&view);
    let overview = get_workspace_overview(&ctx, GetWorkspaceOverviewArgs::default())
        .await
        .unwrap();
    assert!(overview.found);
    let o = overview.item.unwrap();
    assert_eq!(o.symbol_count, 0);
    assert_eq!(o.file_count, 0);
    assert!(o.languages.is_empty());
    // Empty snapshot from a default (all-disabled) config — overview must
    // succeed and carry the structured config-hint instead of erroring.
    let hint = o
        .config_hint
        .as_ref()
        .expect("empty snapshot must carry config_hint");
    assert_eq!(
        hint.kind,
        kenn_query::error::ConfigHintKind::ConfigDisabled,
        "default config has every language disabled"
    );
    assert!(hint.enabled_languages.is_empty());

    // Data-returning tools (search_symbols, find_symbol, ...) MUST surface
    // EMPTY_SNAPSHOT on an empty published snapshot — silent empty results
    // are no longer permitted (mcp-server: Empty-snapshot tools point at
    // config). The structured `data` payload classifies the cause.
    let err = state.open_query().await.err().expect("not ready");
    assert_eq!(err.code, kenn_query::error::QueryErrorCode::EmptySnapshot);
    let data = err.data.expect("EMPTY_SNAPSHOT must carry data payload");
    // `kenn_subcode = "EMPTY_SNAPSHOT"` is added by the wire serializer;
    // QueryError::data carries only the classifier payload.
    assert_eq!(data["kind"], "config-disabled");
    assert!(err.message.contains("kenn.toml"));
    for lang in ["csharp", "rust", "typescript", "python"] {
        assert!(
            err.message.contains(lang),
            "message should list `{lang}`: {}",
            err.message
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn wait_for_index_returns_settled_when_ready() {
    // A bootstrapped Ready snapshot (no reindex in flight) is settled, so
    // `wait_for_index` returns promptly with `timed_out: false`.
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("kenn.toml"),
        "[language.csharp]\nenabled = false\n",
    )
    .unwrap();
    run_cli(dir.path(), &["init"]);
    run_cli(dir.path(), &["index"]);

    let state = ServerState::new(dir.path());
    state.bootstrap().await;

    let start = std::time::Instant::now();
    let resp = wait_for_index(&state, WaitForIndexArgs::default())
        .await
        .unwrap();
    let item = resp.item.unwrap();
    assert!(!item.timed_out, "ready is a settled state");
    assert_eq!(item.status.state, "ready");
    assert!(
        start.elapsed() < std::time::Duration::from_millis(500),
        "settled wait should return promptly"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn uninitialized_workspace_suggests_kenn_init() {
    // Fresh project: never ran `kenn init`, so no `kenn.toml` exists.
    // Index publishes an empty snapshot (default config), and the
    // config-hint must say `not-initialized` and point at `kenn init`
    // rather than the misleading "enable a language in kenn.toml".
    let dir = TempDir::new().unwrap();
    run_cli(dir.path(), &["index"]);
    assert!(
        !dir.path().join("kenn.toml").exists(),
        "test precondition: kenn.toml must be absent"
    );

    let state = ServerState::new(dir.path());
    state.bootstrap().await;
    let view = state.open_query_allow_empty().expect("ready");
    let ctx = state.query_ctx(&view);

    let overview = get_workspace_overview(&ctx, GetWorkspaceOverviewArgs::default())
        .await
        .unwrap();
    let hint = overview
        .item
        .unwrap()
        .config_hint
        .expect("empty snapshot must carry config_hint");
    assert_eq!(hint.kind, kenn_query::error::ConfigHintKind::NotInitialized);
    // The overview hint must carry the actionable suggestion inline — not
    // just the bare `kind` — so an agent orienting via get_workspace_overview
    // is told to run `kenn init` rather than inventing "index not built".
    assert!(
        hint.suggestion.contains("kenn init"),
        "overview config_hint.suggestion should name the action: {}",
        hint.suggestion
    );

    let err = state.open_query().await.err().expect("not ready");
    assert_eq!(err.code, QueryErrorCode::EmptySnapshot);
    let data = err.data.expect("EMPTY_SNAPSHOT must carry data payload");
    assert_eq!(data["kind"], "not-initialized");
    assert!(
        err.message.contains("kenn init"),
        "message should suggest `kenn init`: {}",
        err.message
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "single end-to-end stdio handshake — splitting it would obscure the linear protocol flow"
)]
fn mcp_server_lists_all_tools_over_stdio() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command as Sc, Stdio};

    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("kenn.toml"),
        "[language.csharp]\nenabled = false\n",
    )
    .unwrap();
    run_cli(dir.path(), &["init"]);
    run_cli(dir.path(), &["index"]);

    let cli = assert_cmd::cargo::cargo_bin("kenn");
    let mut child = Sc::new(cli)
        .arg("--workspace")
        .arg(dir.path())
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let stderr_handle = std::thread::spawn(move || {
        use std::io::Read;
        let mut s = String::new();
        let mut r = stderr;
        r.read_to_string(&mut s).unwrap();
        s
    });
    let mut reader = BufReader::new(stdout);

    let init = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}
        }
    });
    writeln!(stdin, "{init}").unwrap();
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.is_empty() {
        drop(stdin);
        child.wait().unwrap();
        let stderr_dump = stderr_handle.join().unwrap_or_default();
        panic!("server EOF before initialize response. stderr:\n{stderr_dump}");
    }
    let init_resp: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_else(|e| {
        let stderr_dump = stderr_handle.join().unwrap_or_default();
        panic!("init parse failed: {e} | line: {line:?} | stderr:\n{stderr_dump}");
    });
    assert!(
        init_resp.get("result").is_some(),
        "initialize failed: {init_resp}"
    );

    let inited = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    writeln!(stdin, "{inited}").unwrap();

    let list = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    writeln!(stdin, "{list}").unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let resp: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    let tools = resp["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("no tools array: {resp}"));
    // MCP-pagination contract: tools/list returns `nextCursor` ONLY
    // when more tools follow. The whole kenn tool set fits in one
    // server-decided page, so the field must be absent. If this
    // ever fails after growing the tool set past the rmcp default
    // page size, walk the cursor instead of bumping the assertion.
    let result = resp["result"].as_object().expect("result object");
    assert!(
        !result.contains_key("nextCursor"),
        "tools/list emitted nextCursor on a single-page response: {resp}"
    );
    // Presence check rather than a count assertion — the list is
    // expected to grow over time; rename-on-every-tool-add is noise.
    // Each entry below is a tool we explicitly require to ship.
    let names: Vec<String> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    for required in [
        "search_symbols",
        "find_symbol",
        "get_symbol",
        "find_similar",
        "find_at_location",
        "list_callers",
        "list_callees",
        "list_implementers",
        "list_overrides",
        "list_usages",
        "find_usages",
        "list_correspondences",
        "list_in_scope",
        "list_imports",
        "list_module_files",
        // The five atlas axes. Required by the `mcp-server` spec
        // ("Atlas axis read tools"), but unregistered until the
        // `split-query-from-mcp` change: they were proven by the CLI and
        // blocked only by the crate-boundary ambiguity that change removed.
        "list_packages",
        "list_domains",
        "list_contracts",
        "list_documents",
        "list_tables",
        "get_workspace_overview",
        "get_index_status",
        "wait_for_index",
        "semantic_search",
        "get_source",
        "get_finding",
        "search_findings",
        "store_finding",
        "merge_findings",
        "find_predecessors",
        "find_successors",
        "reindex",
        "watch_start",
        "watch_stop",
        "debug_env",
    ] {
        assert!(
            names.contains(&required.to_string()),
            "missing tool {required} in {names:?}"
        );
    }

    drop(stdin);
    child.wait().unwrap();
}

/// Every tool that takes an entity reference rejects an unknown one with
/// a structured error rather than a silently-empty result.
///
/// Per the `mcp-server: Empty-snapshot tools point at config` requirement,
/// data-returning tools on an EMPTY snapshot surface `EMPTY_SNAPSHOT` BEFORE
/// any tool-specific input validation can run. The validation paths
/// (`InvalidInput` on unknown references) only fire on populated
/// snapshots; the few writer/findings tools whose contract still applies
/// against an empty code graph keep their `InvalidInput` behavior
/// because `with_findings` does not gate on snapshot emptiness.
#[tokio::test(flavor = "multi_thread")]
async fn tools_error_on_unknown_references() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("kenn.toml"),
        "[language.csharp]\nenabled = false\n",
    )
    .unwrap();
    run_cli(dir.path(), &["init"]);
    run_cli(dir.path(), &["index"]);

    let state = ServerState::new(dir.path());
    state.bootstrap().await;

    // Since queries moved onto `QueryCtx`, the empty-snapshot gate fires when
    // the snapshot is OPENED rather than inside each tool. Same error, one step
    // earlier — and now it guards every migrated query at once instead of each
    // one re-running the check. A caller cannot reach a query without it.
    assert_eq!(
        state
            .open_query()
            .await
            .err()
            .expect("empty snapshot is refused")
            .code,
        QueryErrorCode::EmptySnapshot,
        "opening a query over an empty workspace is refused with the config hint"
    );

    // BEHAVIOR CHANGE, recorded here deliberately. This test used to assert
    // that an empty `file_path` returned INVALID_INPUT even on an empty
    // snapshot, because `find_at_location` validated its argument before
    // reaching `with_db`. A migrated query cannot be called at all without a
    // context, and obtaining one is what fails — so on an empty or not-yet-
    // Ready snapshot the caller now sees EMPTY_SNAPSHOT / INDEX_UNAVAILABLE
    // where it previously saw INVALID_INPUT.
    //
    // That ordering matches what `mcp-server` already requires ("tools other
    // than get_index_status fail fast while not Ready") and it is the same
    // ordering the CLI has always had. Argument validation is unchanged and
    // still runs first once a snapshot is actually open — asserted in the
    // symbol-search suite, which has one.

    // Findings-only tools go through `with_findings` (no empty-snapshot
    // gate, because findings are valid against an empty code graph), so
    // the InvalidInput contract for unknown ids still applies.
    let view = state.open_query_allow_empty().expect("ready");
    let ctx = state.query_ctx(&view);
    let err = find_predecessors(
        &ctx,
        &FindingDagArgs {
            id: "fnd_does-not-exist".into(),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, QueryErrorCode::InvalidInput);

    let err = merge_findings(
        &ctx,
        &MergeFindingsArgs {
            ids: vec!["fnd_aaa".into(), "fnd_bbb".into()],
            text: "synthesis".into(),
            tags: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, QueryErrorCode::InvalidInput);
    assert!(
        err.message.contains("fnd_aaa") && err.message.contains("fnd_bbb"),
        "both unknown ids reported: {}",
        err.message,
    );

    let err = store_finding(
        &ctx,
        &StoreFindingArgs {
            text: "a claim".into(),
            parent_ids: Some(vec![
                "fnd_missing".into(),
                "fnd_also-missing".into(),
                "csharp:cs:SomeNode".into(),
            ]),
            tags: None,
            anchors: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, QueryErrorCode::InvalidInput);
    assert!(
        err.message.contains("fnd_missing") && err.message.contains("fnd_also-missing"),
        "both unknown finding parents reported: {}",
        err.message,
    );
    assert!(
        !err.message.contains("csharp:cs:SomeNode"),
        "code-node parent is accepted, not flagged: {}",
        err.message,
    );
}
