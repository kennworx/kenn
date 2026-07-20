//! Integration coverage for the two symbol-search MCP tools — `find_symbol`
//! (tiered literal lookup) and `search_symbols` (blended ranking) — driven
//! through `ServerState` against an in-process corpus.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kenn_mcp::state::LifecycleState;
use kenn_mcp::tools::{
    find_symbol, search_symbols, FindSymbolArgs, SearchSymbolsArgs, ServerState,
};
use kenn_mcp::types::{RankedSymbolRef, SearchHitRef};
use kenn_mcp::{snapshot_id_from_timestamp, McpErrorCode, Pagination};
use kenn_model::{FileDocsRecord, FileRecord, Kind, Language, PackageRecord, SymbolRecord};
use kenn_store::api::WriteBatch;
use kenn_store::{open_writer, reader_from_writer, DbReader, DbWriter, WriterOptions};
use tempfile::TempDir;

const PKG: u32 = 1;
const FILE: u32 = 1;

/// Unwrap a `search_symbols` hit as a symbol — the fixtures here have no
/// file-level docs, so every hit must be a symbol.
fn hit_sym(h: &SearchHitRef) -> &RankedSymbolRef {
    match h {
        SearchHitRef::Symbol(s) => Some(s),
        SearchHitRef::File(_) => None,
    }
    .expect("expected a symbol hit, got a file hit")
}

fn sym(id: u32, name: &str) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: format!("cs:{name}"),
        language: Language::Csharp,
        pkg_id: PKG,
        kind: Kind::Class,
        name: name.into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

async fn build_corpus(dir: &Path) -> DbWriter {
    let writer = open_writer(dir, WriterOptions::default())
        .await
        .expect("open_writer");
    let batch = WriteBatch {
        packages: vec![PackageRecord {
            id: PKG,
            name: "search-test".into(),
            version: "0.0.0".into(),
            manager: "cargo".into(),
            external: false,
        }],
        files: vec![FileRecord {
            id: FILE,
            path: "src/Orders.cs".into(),
            language: Language::Csharp,
            test: false,
            external: false,
            content_hash: 0,
        }],
        symbols: vec![
            sym(101, "OrderHandler"),
            sym(102, "IOrderHandler"),
            sym(103, "NewOrderHandler"),
            sym(104, "OrderRepository"),
            sym(105, "CreateOrder"),
            sym(106, "ShipmentTracker"),
        ],
        symbol_docs: Vec::new(),
        file_docs: Vec::new(),
        defs: Vec::new(),
        edges: Vec::new(),
    };
    // `finalize` rebuilds the Lance store + search indexes; search is
    // only meaningful afterward.
    writer.write_batch(&batch).await.expect("write_batch");
    writer.finalize().await.expect("finalize");
    writer
}

/// Build a `Ready` server over an in-process reader — no CLI, no
/// published snapshot.
fn ready_state(workspace: &Path, reader: DbReader) -> ServerState {
    let state = ServerState::new(workspace);
    // Synthetic snapshot path → ReaderMarker pins a synthetic registry
    // entry; never visible to a real GC sweep because no Store points
    // at this directory.
    let snap_path = PathBuf::from("in-process");
    let store = kenn_store::Store::open_default(workspace).expect("store");
    let pin = kenn_store::readers::register_reader(&store, &snap_path).expect("pin");
    *state.lifecycle.write().expect("lifecycle lock") = LifecycleState::Ready {
        snapshot_path: snap_path,
        snapshot_id: snapshot_id_from_timestamp("symbol-search-test"),
        indexed_at: "symbol-search-test".into(),
        read: arc_swap::ArcSwap::from(Arc::new(kenn_mcp::state::ReaderBinding::new(reader, pin))),
        fallback_from_parent: false,
        reindex: None,
        run_meta: None,
    };
    state
}

fn search_args(page_size: u32, cursor: Option<String>) -> SearchSymbolsArgs {
    SearchSymbolsArgs {
        query: "order".into(),
        filters: None,
        pagination: Some(Pagination {
            page_size: Some(page_size),
            cursor,
        }),
    }
}

/// `find_symbol` ranks the exact name first and tags every row with its
/// match tier.
#[tokio::test(flavor = "multi_thread")]
async fn find_symbol_ranks_exact_then_substring() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );

    let resp = find_symbol(
        &state,
        &FindSymbolArgs {
            name: "OrderHandler".into(),
            kind: None,
            page_size: None,
            include_tests: None,
            include_external: None,
        },
    )
    .await
    .expect("find_symbol");

    assert_eq!(resp.items[0].base.name, "OrderHandler");
    assert_eq!(resp.items[0].match_kind, "exact");
    for row in &resp.items[1..] {
        assert_ne!(
            row.match_kind, "exact",
            "only one symbol is named OrderHandler"
        );
    }
}

/// `search_symbols` returns blended-ranked rows ordered by score.
#[tokio::test(flavor = "multi_thread")]
async fn search_symbols_returns_blended_ranking() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );

    let resp = search_symbols(&state, &search_args(50, None))
        .await
        .expect("search_symbols");
    assert!(
        resp.items.len() >= 4,
        "query 'order' matches several symbols, got {}",
        resp.items.len()
    );
    for w in resp.items.windows(2) {
        assert!(
            hit_sym(&w[0]).score >= hit_sym(&w[1]).score,
            "rows are ranked by score DESC"
        );
    }
}

/// `search_symbols` paginates: page 1 hands back a cursor, later pages
/// continue with no overlap, and the union covers the full set.
#[tokio::test(flavor = "multi_thread")]
async fn search_symbols_paginates_without_gaps() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );

    let full = search_symbols(&state, &search_args(50, None))
        .await
        .expect("full page");
    let full_ids: Vec<String> = full.items.iter().map(|r| hit_sym(r).id.clone()).collect();

    let mut paged: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let resp = search_symbols(&state, &search_args(2, cursor.clone()))
            .await
            .expect("page");
        paged.extend(resp.items.iter().map(|r| hit_sym(r).id.clone()));
        match resp.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(
        paged, full_ids,
        "cursor-paged traversal must equal the single-page order"
    );
}

/// A cursor minted against an evicted (or never-existed) cache entry
/// is rejected as stale. With cache-backed top-K pagination the
/// staleness signal is "`cache_id` not in the result cache" — produced
/// either by snapshot rotation (which clears the cache) or LRU
/// eviction. Per `mcp-pagination-spec-alignment` §2.1 the wire shape
/// is JSON-RPC `-32602` with `data.kenn_subcode = "STALE_CURSOR"`.
#[tokio::test(flavor = "multi_thread")]
async fn search_symbols_rejects_a_stale_cursor() {
    use kenn_mcp::encode_topk_cursor;
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );

    // A TopK cursor with a cache_id that was never put. Cache miss
    // surfaces as STALE_CURSOR.
    let stale_cursor = encode_topk_cursor([0xFE; 16], 0);
    let err = search_symbols(&state, &search_args(2, Some(stale_cursor)))
        .await
        .expect_err("stale cursor must be rejected");
    assert_eq!(err.code, McpErrorCode::StaleCursor);
    // Both stale and malformed cursors map to the same JSON-RPC code.
    assert_eq!(err.code.json_rpc_code(), -32602);
}

/// Per the MCP pagination spec, a length-mutated cursor decodes-fails
/// rather than reaching the snapshot check. Maps to `InvalidInput`
/// (`kenn_subcode = "INVALID_CURSOR"`, wire code `-32602`).
#[tokio::test(flavor = "multi_thread")]
async fn search_symbols_rejects_a_malformed_cursor() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );

    // Truncated — wrong byte count after base64 decode.
    let err = search_symbols(&state, &search_args(2, Some("abc".to_string())))
        .await
        .expect_err("malformed cursor must be rejected");
    assert_eq!(err.code, McpErrorCode::InvalidInput);
    assert_eq!(err.code.json_rpc_code(), -32602);
}

/// The final page of a paginated walk explicitly carries no
/// continuation cursor. Per the MCP pagination spec, the missing
/// cursor IS the end-of-stream signal — the agent stops paging.
/// (Re-issuing the previous cursor would re-fetch the same page, not
/// "go past the end"; there's no cursor that points "after exhausted.")
#[tokio::test(flavor = "multi_thread")]
async fn search_symbols_final_page_emits_no_cursor() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );

    let mut cursor: Option<String> = None;
    for _ in 0..100 {
        let resp = search_symbols(&state, &search_args(2, cursor.clone()))
            .await
            .expect("page");
        if let Some(c) = resp.next {
            cursor = Some(c);
        } else {
            // Final page reached: no continuation cursor, as the spec
            // requires. Returning here without `panic` is the success path.
            return;
        }
    }
    panic!("pagination did not terminate after 100 pages");
}

/// A paginated `search_symbols` walk reuses a single materialized
/// result set across pages — i.e. the cache amortizes the embedding +
/// Lance probe work past page 1. Without a reader-call spy this test
/// asserts the observable invariant: walking with `page_size=2`
/// against a 6-symbol corpus reaches exactly the same item set as
/// `page_size=30` single-shot, in the same order, even though pages
/// 2..N come from the cache rather than re-querying the reader. If
/// the cache were missing, pagination would either re-run the
/// embedder per page (correctness still holds) or fail to find a
/// stable cursor across the re-run; this test pins the working
/// behaviour after the cache refactor.
#[tokio::test(flavor = "multi_thread")]
async fn search_symbols_caches_first_call() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );

    let single = search_symbols(&state, &search_args(30, None))
        .await
        .expect("single-shot");
    let single_ids: Vec<String> = single.items.iter().map(|r| hit_sym(r).id.clone()).collect();
    assert!(single.next.is_none(), "page_size=30 must be single-shot");
    assert!(
        single_ids.len() >= 4,
        "corpus must have enough hits to paginate"
    );

    let mut paged_ids: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let resp = search_symbols(&state, &search_args(2, cursor.clone()))
            .await
            .expect("page");
        paged_ids.extend(resp.items.iter().map(|r| hit_sym(r).id.clone()));
        pages += 1;
        assert!(pages < 50, "pagination must terminate");
        match resp.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(
        paged_ids, single_ids,
        "paged walk must reproduce single-shot order; cache miss would skip rows or re-order"
    );
    assert!(
        pages >= 2,
        "with page_size=2 vs 6+ matches, must take ≥2 pages"
    );
}

/// Snapshot rotation in production clears the result cache via the
/// rotation hook (`indexing.rs` calls `state.search_*_cache.clear()`).
/// Continuation cursors against pre-rotation entries surface as
/// `STALE_CURSOR`. This test drives the same effect via the public
/// `ServerState::clear_result_caches()` so the cache-miss → stale
/// path is covered without standing up a real reindex.
#[tokio::test(flavor = "multi_thread")]
async fn search_symbols_cursor_stale_after_clear() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );

    let page1 = search_symbols(&state, &search_args(2, None))
        .await
        .expect("first page");
    let cursor = page1
        .next
        .expect("first page must emit a cursor against this corpus");

    // Simulate snapshot rotation by clearing the cache.
    state.clear_result_caches();

    let err = search_symbols(&state, &search_args(2, Some(cursor)))
        .await
        .expect_err("continuation after cache clear must fail");
    assert_eq!(err.code, McpErrorCode::StaleCursor);
}

/// Task 5.4: a C# file header (no owning symbol) is BM25-searchable and
/// surfaces through `search_symbols` as a FILE hit, tagged `result_kind:
/// "file"`, distinct from symbol hits.
#[tokio::test(flavor = "multi_thread")]
async fn search_symbols_returns_file_hit_for_header_token() {
    let dir = TempDir::new().unwrap();
    let writer = open_writer(dir.path(), WriterOptions::default())
        .await
        .expect("open_writer");
    let batch = WriteBatch {
        packages: vec![PackageRecord {
            id: PKG,
            name: "fh-test".into(),
            version: "0.0.0".into(),
            manager: "cargo".into(),
            external: false,
        }],
        files: vec![FileRecord {
            id: FILE,
            path: "src/Payments.cs".into(),
            language: Language::Csharp,
            test: false,
            external: false,
            content_hash: 0,
        }],
        // Symbol name deliberately shares no n-grams with the query token.
        symbols: vec![sym(101, "PaymentGateway")],
        symbol_docs: Vec::new(),
        file_docs: vec![FileDocsRecord {
            file_id: FILE,
            doc: "Subsystem header: the zqxsettlementtoken orchestration module.".into(),
        }],
        defs: Vec::new(),
        edges: Vec::new(),
    };
    writer.write_batch(&batch).await.expect("write_batch");
    writer.finalize().await.expect("finalize");
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );

    let resp = search_symbols(
        &state,
        &SearchSymbolsArgs {
            query: "zqxsettlementtoken".into(),
            filters: None,
            pagination: Some(Pagination {
                page_size: Some(50),
                cursor: None,
            }),
        },
    )
    .await
    .expect("search_symbols");

    let file_hit = resp
        .items
        .iter()
        .find_map(|h| match h {
            SearchHitRef::File(f) => Some(f),
            SearchHitRef::Symbol(_) => None,
        })
        .expect("the file-header token must surface as a file hit");
    assert_eq!(file_hit.kind, "file");
    assert_eq!(file_hit.path, "src/Payments.cs");
}
