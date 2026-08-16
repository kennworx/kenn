//! Integration coverage for navigation MCP tools — `list_usages` and
//! `list_imports`. Driven through `ServerState` against a fixture with
//! `Calls` / `Imports` edges so the inner edge-kind loops are exercised.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kenn_mcp::state::LifecycleState;
use kenn_mcp::tools::ServerState;
use kenn_model::{
    EdgeKind, EdgeProperties, EdgeRecord, FileRecord, ImportKind, Kind, Language, PackageRecord,
    SymbolRecord,
};
use kenn_query::snapshot_id_from_timestamp;
use kenn_query::{
    list_imports, list_module_files, list_usages, ByIdArgs, ImportDirectionArg, ListImportsArgs,
    ListUsagesArgs,
};
use kenn_store::api::WriteBatch;
use kenn_store::{open_writer, reader_from_writer, DbReader, DbWriter, WriterOptions};
use tempfile::TempDir;

const PKG_A: u32 = 1;
const PKG_B: u32 = 2;
const FILE: u32 = 1;
const HANDLER_ID: u32 = 101;
const REPO_ID: u32 = 102;
const SHIPMENT_ID: u32 = 103;
/// A module that CONTAINS the corpus file — `list_module_files` walks
/// `Contains` edges from a module symbol to file ids, so without one the
/// query has nothing to return and its test would pass vacuously.
const MODULE_ID: u32 = 104;

fn sym(id: u32, pkg_id: u32, name: &str, kind: Kind) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: format!("cs:{name}"),
        language: Language::Csharp,
        pkg_id,
        kind,
        name: name.into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

fn pkg(id: u32, name: &str) -> PackageRecord {
    PackageRecord {
        id,
        name: name.into(),
        version: "0.0.0".into(),
        manager: "cargo".into(),
        external: false,
    }
}

async fn build_corpus(dir: &Path) -> DbWriter {
    let writer = open_writer(dir, WriterOptions::default())
        .await
        .expect("open_writer");
    let batch = WriteBatch {
        packages: vec![pkg(PKG_A, "kenn-test-a"), pkg(PKG_B, "kenn-test-b")],
        files: vec![FileRecord {
            id: FILE,
            path: "src/Orders.cs".into(),
            language: Language::Csharp,
            test: false,
            external: false,
            content_hash: 0,
        }],
        symbols: vec![
            sym(HANDLER_ID, PKG_A, "OrderHandler", Kind::Class),
            sym(REPO_ID, PKG_A, "OrderRepository", Kind::Class),
            sym(SHIPMENT_ID, PKG_B, "ShipmentTracker", Kind::Class),
            sym(MODULE_ID, PKG_A, "Orders", Kind::Module),
        ],
        symbol_docs: Vec::new(),
        file_docs: Vec::new(),
        defs: Vec::new(),
        edges: vec![
            // The module contains the corpus file — what `list_module_files` walks.
            EdgeRecord {
                src_id: MODULE_ID,
                target_id: FILE,
                properties: EdgeProperties::Contains,
            },
            // OrderHandler calls OrderRepository (Calls edge, inbound on Repo)
            EdgeRecord {
                src_id: HANDLER_ID,
                target_id: REPO_ID,
                properties: EdgeProperties::Calls,
            },
            // OrderHandler uses ShipmentTracker as a type (TypeUse edge)
            EdgeRecord {
                src_id: HANDLER_ID,
                target_id: SHIPMENT_ID,
                properties: EdgeProperties::TypeUse,
            },
            // pkg A imports pkg B (Imports edge between packages — list_imports
            // works on modules; here we put it on symbols so the reader path
            // is exercised at all).
            EdgeRecord {
                src_id: HANDLER_ID,
                target_id: SHIPMENT_ID,
                properties: EdgeProperties::Imports {
                    kind: ImportKind::Explicit,
                },
            },
        ],
    };
    writer.write_batch(&batch).await.expect("write_batch");
    writer.finalize().await.expect("finalize");
    writer
}

fn ready_state(workspace: &Path, reader: DbReader) -> ServerState {
    let state = ServerState::new(workspace);
    let snap_path = PathBuf::from("in-process");
    let store = kenn_store::Store::open_default(workspace).expect("store");
    let pin = kenn_store::readers::register_reader(&store, &snap_path).expect("pin");
    *state.lifecycle.write().expect("lifecycle lock") = LifecycleState::Ready {
        snapshot_path: snap_path,
        snapshot_id: snapshot_id_from_timestamp("navigation-test"),
        indexed_at: "navigation-test".into(),
        read: arc_swap::ArcSwap::from(Arc::new(kenn_mcp::state::ReaderBinding::new(reader, pin))),
        fallback_from_parent: false,
        reindex: None,
        run_meta: None,
    };
    state
}

/// `list_usages` aggregates inbound edges across multiple edge kinds.
/// With default kinds `[Calls, TypeUse, FieldAccess, Instantiates]` and
/// the fixture's two inbound edges on `REPO`/`SHIPMENT` (`Calls` + `TypeUse`),
/// each target's response should carry the right `via_edge_kind` tag.
#[tokio::test(flavor = "multi_thread")]
async fn list_usages_aggregates_inbound_edges_across_kinds() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    // REPO has 1 inbound Calls edge from HANDLER.
    let resp = list_usages(
        &ctx,
        &ListUsagesArgs {
            id: "cs:OrderRepository".into(),
            edge_kinds: None, // default = Calls/TypeUse/FieldAccess/Instantiates
            op_filter: None,
            filters: None,
            pagination: None,
        },
    )
    .await
    .expect("list_usages");
    assert!(
        resp.items
            .iter()
            .any(|it| it.via_edge_kind == Some(EdgeKind::Calls)),
        "expected a Calls usage in the response"
    );
    assert!(!resp.items.is_empty(), "expected ≥1 usage");
}

/// `list_imports` enumerates Imports edges in the requested direction.
/// The fixture has one Imports edge from HANDLER → SHIPMENT; outbound
/// from HANDLER returns 1, inbound on SHIPMENT returns 1.
#[tokio::test(flavor = "multi_thread")]
async fn list_imports_returns_outbound_and_inbound() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let outbound = list_imports(
        &ctx,
        &ListImportsArgs {
            id: "cs:OrderHandler".into(),
            direction: ImportDirectionArg::Outbound,
            kind: None,
            filters: None,
            pagination: None,
        },
    )
    .await
    .expect("list_imports outbound");
    assert!(
        !outbound.items.is_empty(),
        "outbound items count {}",
        outbound.items.len()
    );

    let inbound = list_imports(
        &ctx,
        &ListImportsArgs {
            id: "cs:ShipmentTracker".into(),
            direction: ImportDirectionArg::Inbound,
            kind: None,
            filters: None,
            pagination: None,
        },
    )
    .await
    .expect("list_imports inbound");
    assert!(
        !inbound.items.is_empty(),
        "inbound items count {}",
        inbound.items.len()
    );

    // `both` direction returns rows from both sides plus a per-row
    // direction tag.
    let both = list_imports(
        &ctx,
        &ListImportsArgs {
            id: "cs:OrderHandler".into(),
            direction: ImportDirectionArg::Both,
            kind: None,
            filters: None,
            pagination: None,
        },
    )
    .await
    .expect("list_imports both");
    assert!(both.items.len() >= outbound.items.len());
}

/// Sanity: `list_imports` against a symbol with no Imports edges
/// returns a zero total.
#[tokio::test(flavor = "multi_thread")]
async fn list_imports_zero_total_on_unused_symbol() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let resp = list_imports(
        &ctx,
        &ListImportsArgs {
            id: "cs:OrderRepository".into(),
            direction: ImportDirectionArg::Outbound,
            kind: None,
            filters: None,
            pagination: None,
        },
    )
    .await
    .expect("list_imports zero");
    assert!(resp.items.is_empty());
}

/// `list_module_files` walks `Contains` edges from a module symbol to file ids.
///
/// Added because the CRAP gate reported the function at 0% coverage once the
/// `QueryCtx` migration un-nested it: nothing in-process had ever called it.
/// The corpus needed a module and a `Contains` edge before the query could
/// return anything — without them the test would pass on an empty list and
/// guard nothing.
#[tokio::test(flavor = "multi_thread")]
async fn list_module_files_returns_the_modules_files() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let resp = list_module_files(
        &ctx,
        &ByIdArgs {
            id: "cs:Orders".into(),
            filters: None,
            pagination: None,
        },
    )
    .await
    .expect("list_module_files");

    assert_eq!(
        resp.items
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        ["src/Orders.cs"],
        "the module's contained file, resolved to its path"
    );
    assert_eq!(resp.items[0].language, Language::Csharp);

    // An id that resolves to no symbol is a loud error, not an empty list.
    let err = list_module_files(
        &ctx,
        &ByIdArgs {
            id: "cs:NoSuchModule".into(),
            filters: None,
            pagination: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, kenn_query::QueryErrorCode::InvalidInput);
}
