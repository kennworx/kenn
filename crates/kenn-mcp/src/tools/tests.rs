use super::*;

use kenn_model::Kind;
use kenn_store::Store;

use crate::error::McpErrorCode;

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

#[tokio::test(flavor = "multi_thread")]
async fn other_tools_index_unavailable_when_no_live() {
    let dir = TempDir::new().unwrap();
    let _ = Store::open_default(dir.path()).unwrap();
    let state = ServerState::new(dir.path());
    let err = get_workspace_overview(&state, GetWorkspaceOverviewArgs::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, McpErrorCode::IndexUnavailable);
}

#[tokio::test(flavor = "multi_thread")]
async fn get_symbol_rejects_empty_id() {
    let dir = TempDir::new().unwrap();
    let state = ServerState::new(dir.path());
    let err = get_symbol(&state, &GetSymbolArgs { id: String::new() })
        .await
        .unwrap_err();
    assert_eq!(err.code, McpErrorCode::InvalidInput);
}

#[tokio::test(flavor = "multi_thread")]
async fn get_symbol_rejects_id_without_language_prefix() {
    let dir = TempDir::new().unwrap();
    let state = ServerState::new(dir.path());
    let err = get_symbol(&state, &GetSymbolArgs { id: "Foo".into() })
        .await
        .unwrap_err();
    assert_eq!(err.code, McpErrorCode::InvalidInput);
}

#[tokio::test(flavor = "multi_thread")]
async fn find_symbol_rejects_empty_name() {
    let dir = TempDir::new().unwrap();
    let state = ServerState::new(dir.path());
    let err = find_symbol(
        &state,
        &FindSymbolArgs {
            name: String::new(),
            kind: None,
            page_size: None,
            include_tests: None,
            include_external: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, McpErrorCode::InvalidInput);
}

#[test]
fn split_public_id_returns_db_language_and_full_id() {
    assert_eq!(
        split_public_id("rs:foo::bar").unwrap(),
        ("rust", "rs:foo::bar")
    );
    assert_eq!(
        split_public_id("cs:Models.Order").unwrap(),
        ("csharp", "cs:Models.Order")
    );
    split_public_id(":empty-lang").unwrap_err();
    split_public_id("no-colon").unwrap_err();
    split_public_id("xx:unknown").unwrap_err();
}

/// `parse_kind` is the tool-argument decoder used by every tool
/// that takes a `kind` filter (`find_symbol`, `search_symbols`, etc.).
/// Round-trip every `Kind` variant through `db_name → parse_kind`
/// and cover the `None` arm with unknown strings.
#[test]
fn parse_kind_decodes_every_variant() {
    for k in [
        Kind::Package,
        Kind::Module,
        Kind::Namespace,
        Kind::Class,
        Kind::Struct,
        Kind::Interface,
        Kind::Trait,
        Kind::Enum,
        Kind::EnumMember,
        Kind::TypeAlias,
        Kind::Method,
        Kind::Function,
        Kind::Constructor,
        Kind::Destructor,
        Kind::Operator,
        Kind::Field,
        Kind::Property,
        Kind::Constant,
        Kind::Variable,
        Kind::Parameter,
        Kind::TypeParameter,
        Kind::Macro,
    ] {
        assert_eq!(parse_kind(k.db_name()), Some(k), "decode {k:?}");
    }
    for unknown in ["", "Class", "klass", "package_", " package"] {
        assert!(parse_kind(unknown).is_none(), "{unknown:?} must not decode");
    }
}

/// `slice_lines` consumes 1-based input per `source-data-model` D1.
/// A symbol whose `def_range.start_line = 1` MUST yield the file's
/// first line — not `""` and not the line above (impossible anyway).
#[test]
fn slice_lines_returns_first_line_for_start_line_1() {
    let content = "first line\nsecond line\nthird line\n";
    assert_eq!(super::slice_lines(content, 1, 1), "first line");
    assert_eq!(super::slice_lines(content, 1, 2), "first line\nsecond line");
    assert_eq!(super::slice_lines(content, 2, 3), "second line\nthird line");
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
