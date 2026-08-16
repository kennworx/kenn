//! Integration coverage for the `find_usages` MCP tool — the fused
//! resolve-then-traverse intent. Driven through `ServerState` against a
//! fixture carrying a symbol, a file (with an inbound `imports` edge), an
//! external attachment stub (with an inbound `links_to` edge), an
//! ambiguous name, and a heavily-referenced target for pagination.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kenn_mcp::state::LifecycleState;
use kenn_mcp::tools::ServerState;
use kenn_model::{
    EdgeKind, EdgeProperties, EdgeRecord, FileRecord, ImportKind, Kind, Language, LinkGrade,
    PackageRecord, SymbolRecord,
};
use kenn_query::snapshot_id_from_timestamp;
use kenn_query::{find_usages, FindUsagesArgs};
use kenn_store::api::WriteBatch;
use kenn_store::{open_writer, reader_from_writer, DbReader, DbWriter, WriterOptions};
use tempfile::TempDir;

const PKG_A: u32 = 1;
const PKG_B: u32 = 2;

const FILE_TS: u32 = 1;
const FILE_MD: u32 = 2;

const HANDLER: u32 = 101;
const REPO: u32 = 102;
const LONER: u32 = 103;
const POPULAR: u32 = 104;
const CALLER1: u32 = 105;
const CALLER2: u32 = 106;
const CALLER3: u32 = 107;
const ZEPHYR_A: u32 = 108;
const ZEPHYR_B: u32 = 109;
const ZUSER1: u32 = 110;
const ZUSER2: u32 = 111;
const MODULE: u32 = 112;
const DOC: u32 = 201;
const ATTACH: u32 = 202;

const ATTACH_PUB: &str = "md:@unresolved/assets/logo.png";

fn sym(id: u32, pkg_id: u32, pub_id: &str, name: &str, kind: Kind, lang: Language) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: pub_id.into(),
        language: lang,
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

fn calls(src: u32, target: u32) -> EdgeRecord {
    EdgeRecord {
        src_id: src,
        target_id: target,
        properties: EdgeProperties::Calls,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "linear test-corpus builder — one node/edge per statement; splitting would scatter the fixture, not clarify it"
)]
async fn build_corpus(dir: &Path) -> DbWriter {
    let writer = open_writer(dir, WriterOptions::default())
        .await
        .expect("open_writer");

    let attach = SymbolRecord {
        id: ATTACH,
        pub_id: ATTACH_PUB.into(),
        language: Language::Markdown,
        pkg_id: 0,
        kind: Kind::Attachment,
        name: "assets/logo.png".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: true,
        test: false,
    };

    let batch = WriteBatch {
        packages: vec![pkg(PKG_A, "kenn-test-a"), pkg(PKG_B, "kenn-test-b")],
        files: vec![
            FileRecord {
                id: FILE_TS,
                path: "src/orders/api.ts".into(),
                language: Language::TypeScript,
                test: false,
                external: false,
                content_hash: 0,
            },
            FileRecord {
                id: FILE_MD,
                path: "docs/auth.md".into(),
                language: Language::Markdown,
                test: false,
                external: false,
                content_hash: 0,
            },
        ],
        symbols: vec![
            sym(
                HANDLER,
                PKG_A,
                "cs:OrderHandler",
                "OrderHandler",
                Kind::Class,
                Language::Csharp,
            ),
            sym(
                REPO,
                PKG_A,
                "cs:OrderRepository",
                "OrderRepository",
                Kind::Class,
                Language::Csharp,
            ),
            sym(
                LONER,
                PKG_A,
                "cs:Loner",
                "Loner",
                Kind::Class,
                Language::Csharp,
            ),
            sym(
                POPULAR,
                PKG_A,
                "cs:PopularRepo",
                "PopularRepo",
                Kind::Class,
                Language::Csharp,
            ),
            sym(
                CALLER1,
                PKG_A,
                "cs:Caller1",
                "Caller1",
                Kind::Class,
                Language::Csharp,
            ),
            sym(
                CALLER2,
                PKG_A,
                "cs:Caller2",
                "Caller2",
                Kind::Class,
                Language::Csharp,
            ),
            sym(
                CALLER3,
                PKG_A,
                "cs:Caller3",
                "Caller3",
                Kind::Class,
                Language::Csharp,
            ),
            sym(
                ZEPHYR_A,
                PKG_A,
                "cs:Acme.Zephyr",
                "Zephyr",
                Kind::Class,
                Language::Csharp,
            ),
            sym(
                ZEPHYR_B,
                PKG_B,
                "cs:Beta.Zephyr",
                "Zephyr",
                Kind::Interface,
                Language::Csharp,
            ),
            sym(
                ZUSER1,
                PKG_A,
                "cs:ZUser1",
                "ZUser1",
                Kind::Class,
                Language::Csharp,
            ),
            sym(
                ZUSER2,
                PKG_A,
                "cs:ZUser2",
                "ZUser2",
                Kind::Class,
                Language::Csharp,
            ),
            sym(
                MODULE,
                PKG_A,
                "ts:orders/index",
                "index",
                Kind::Module,
                Language::TypeScript,
            ),
            sym(
                DOC,
                0,
                "md:docs/auth.md",
                "Auth",
                Kind::Document,
                Language::Markdown,
            ),
            attach,
        ],
        symbol_docs: Vec::new(),
        file_docs: Vec::new(),
        defs: Vec::new(),
        edges: vec![
            calls(HANDLER, REPO),
            calls(CALLER1, POPULAR),
            calls(CALLER2, POPULAR),
            calls(CALLER3, POPULAR),
            calls(ZUSER1, ZEPHYR_A),
            calls(ZUSER2, ZEPHYR_B),
            // A module imports the ts file → inbound `imports` edge on the file.
            EdgeRecord {
                src_id: MODULE,
                target_id: FILE_TS,
                properties: EdgeProperties::Imports {
                    kind: ImportKind::Explicit,
                },
            },
            // A doc links the asset → inbound `links_to` edge on the stub.
            EdgeRecord {
                src_id: DOC,
                target_id: ATTACH,
                properties: EdgeProperties::LinksTo {
                    grade: LinkGrade::Dangling,
                    relation: String::new(),
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
        snapshot_id: snapshot_id_from_timestamp("find-usages-test"),
        indexed_at: "find-usages-test".into(),
        read: arc_swap::ArcSwap::from(Arc::new(kenn_mcp::state::ReaderBinding::new(reader, pin))),
        fallback_from_parent: false,
        reindex: None,
        run_meta: None,
    };
    state
}

async fn state_with_corpus(dir: &TempDir) -> ServerState {
    let writer = build_corpus(dir.path()).await;
    ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    )
}

fn args(query: &str) -> FindUsagesArgs {
    FindUsagesArgs {
        query: query.into(),
        kind: None,
        path: None,
        package: None,
        language: None,
        edge_kinds: None,
        include_external: None,
        include_tests: None,
        page_size: None,
        cursor: None,
    }
}

/// A plain name resolves through the name index and returns the inbound
/// `Calls` reference, tagged with the resolved target — in one call.
#[tokio::test(flavor = "multi_thread")]
async fn name_to_references_one_call() {
    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let resp = find_usages(&ctx, &args("OrderRepository"))
        .await
        .expect("find_usages");
    // The name index may also fuzzy-match OrderHandler (shared trigrams);
    // either way the Calls reference is returned, tagged with its target.
    assert!(resp.targets >= 1);
    assert!(!resp.truncated);
    assert!(
        resp.items
            .iter()
            .any(|u| u.reference.via_edge_kind == Some(EdgeKind::Calls)
                && u.reference.name == "OrderHandler"
                && u.target == "cs:OrderRepository"),
        "expected a Calls reference from OrderHandler tagged with cs:OrderRepository: {:?}",
        resp.items
    );
}

/// A workspace-relative path resolves via the file lookup (not the name
/// index) and the default edge set surfaces its inbound `imports`.
#[tokio::test(flavor = "multi_thread")]
async fn path_resolves_to_file_and_surfaces_imports() {
    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let resp = find_usages(&ctx, &args("src/orders/api.ts"))
        .await
        .expect("find_usages");
    assert_eq!(resp.targets, 1);
    assert!(
        resp.items
            .iter()
            .any(|u| u.reference.via_edge_kind == Some(EdgeKind::Imports)
                && u.target == "src/orders/api.ts"),
        "default edge set must surface the file's `imports` importers: {:?}",
        resp.items
    );
}

/// An asset path resolves to its external attachment stub and lists the
/// `links_to` references to it.
#[tokio::test(flavor = "multi_thread")]
async fn asset_path_lists_links_to_references() {
    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let resp = find_usages(&ctx, &args("assets/logo.png"))
        .await
        .expect("find_usages");
    assert_eq!(resp.targets, 1);
    assert!(
        resp.items
            .iter()
            .any(|u| u.reference.via_edge_kind == Some(EdgeKind::LinksTo)
                && u.reference.name == "Auth"
                && u.target == ATTACH_PUB),
        "asset stub should surface its links_to referencers: {:?}",
        resp.items
    );
}

/// A `pub_id` query is used directly (resolution skipped) and yields one
/// target.
#[tokio::test(flavor = "multi_thread")]
async fn pub_id_query_resolves_directly() {
    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let resp = find_usages(&ctx, &args("cs:OrderRepository"))
        .await
        .expect("find_usages");
    assert_eq!(resp.targets, 1);
    assert_eq!(resp.total_targets, 1);
    assert!(resp.items.iter().all(|u| u.target == "cs:OrderRepository"));
    assert!(!resp.items.is_empty());
}

/// An ambiguous name returns a single flat list, each row tagged with the
/// resolved target it belongs to, with `next: null` and truncation
/// reported (here `truncated == false` since 2 ≤ cap).
#[tokio::test(flavor = "multi_thread")]
async fn ambiguous_name_flat_tagged_list_no_pagination() {
    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let resp = find_usages(&ctx, &args("Zephyr"))
        .await
        .expect("find_usages");
    assert!(resp.next.is_none(), "multi-target must not paginate");
    assert!(resp.targets >= 2, "both Zephyr symbols resolve");
    assert!(!resp.truncated, "2 targets is under the cap");
    // Flat list interleaves rows for both targets.
    assert!(resp.items.iter().any(|u| u.target == "cs:Acme.Zephyr"));
    assert!(resp.items.iter().any(|u| u.target == "cs:Beta.Zephyr"));
}

/// A narrowing filter pins a single target out of an ambiguous name,
/// which then paginates.
#[tokio::test(flavor = "multi_thread")]
async fn kind_filter_pins_single_target() {
    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let mut a = args("Zephyr");
    a.kind = Some(vec![Kind::Interface]);
    let resp = find_usages(&ctx, &a).await.expect("find_usages");
    assert_eq!(resp.targets, 1);
    assert!(
        resp.items.iter().all(|u| u.target == "cs:Beta.Zephyr"),
        "kind=interface pins the interface Zephyr only: {:?}",
        resp.items
    );
}

/// A query that matches no node returns an empty result, not an error.
#[tokio::test(flavor = "multi_thread")]
async fn missing_query_is_empty_not_error() {
    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let resp = find_usages(&ctx, &args("DefinitelyNotAName"))
        .await
        .expect("find_usages must not error on a no-match query");
    assert!(resp.items.is_empty());
    assert_eq!(resp.targets, 0);
    assert!(resp.next.is_none());
}

/// A real entity with zero incoming references returns empty (used
/// nowhere), not an error.
#[tokio::test(flavor = "multi_thread")]
async fn unreferenced_real_symbol_is_empty() {
    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let resp = find_usages(&ctx, &args("Loner"))
        .await
        .expect("find_usages");
    assert_eq!(resp.targets, 1, "the symbol resolves");
    assert!(resp.items.is_empty(), "but nothing references it");
    assert!(resp.next.is_none());
}

/// Explicit `edge_kinds` overrides the default set.
#[tokio::test(flavor = "multi_thread")]
async fn explicit_edge_kinds_narrows_traversal() {
    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let mut a = args("OrderRepository");
    a.edge_kinds = Some(vec![EdgeKind::TypeUse]);
    let resp = find_usages(&ctx, &a).await.expect("find_usages");
    assert!(
        resp.items.is_empty(),
        "OrderRepository has only a Calls referencer, not TypeUse"
    );
}

/// A single resolved target paginates: the `next` cursor round-trips and
/// walks the rest of the references.
#[tokio::test(flavor = "multi_thread")]
async fn single_target_pagination_round_trips() {
    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let mut a = args("PopularRepo");
    a.edge_kinds = Some(vec![EdgeKind::Calls]);
    a.page_size = Some(2);

    let page1 = find_usages(&ctx, &a).await.expect("page1");
    assert_eq!(page1.items.len(), 2, "first page is full");
    let cursor = page1.next.clone().expect("a next cursor for >1 page");

    let mut a2 = a.clone();
    a2.cursor = Some(cursor);
    let page2 = find_usages(&ctx, &a2).await.expect("page2");
    assert_eq!(page2.items.len(), 1, "remaining reference");
    assert!(page2.next.is_none(), "stream exhausted");

    // Three distinct callers across the two pages, no duplicates.
    let mut names: Vec<String> = page1
        .items
        .iter()
        .chain(page2.items.iter())
        .map(|u| u.reference.name.clone())
        .collect();
    names.sort();
    names.dedup();
    assert_eq!(names, vec!["Caller1", "Caller2", "Caller3"]);
}

/// A stale cursor (snapshot rotated) is rejected with `-32602`-class
/// `InvalidInput`.
#[tokio::test(flavor = "multi_thread")]
async fn stale_cursor_is_rejected() {
    use kenn_query::{encode_usages_cursor, snapshot_id_from_timestamp};

    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let view = state.open_query().await.expect("snapshot opens");
    let ctx = state.query_ctx(&view);

    let rotated = encode_usages_cursor(snapshot_id_from_timestamp("some-other-snapshot"), 0, 0);
    let mut a = args("PopularRepo");
    a.cursor = Some(rotated);
    let err = find_usages(&ctx, &a)
        .await
        .expect_err("stale cursor errors");
    assert_eq!(err.code, kenn_query::QueryErrorCode::StaleCursor);
}
