//! Integration coverage for task 7.3: the existing nav/search MCP tools
//! return `md:` nodes unchanged. Markdown documents/sections are ordinary
//! symbol-space nodes (design D10), so `find_symbol`, `search_symbols`,
//! `list_in_scope`, and `find_at_location` serve them with no md-specific
//! code path — this test pins that.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kenn_mcp::state::LifecycleState;
use kenn_mcp::tools::{
    find_at_location, find_symbol, list_in_scope, search_symbols, ByIdArgs, FindAtLocationArgs,
    FindSymbolArgs, SearchSymbolsArgs, ServerState,
};
use kenn_mcp::types::SearchHitRef;
use kenn_mcp::{snapshot_id_from_timestamp, Pagination};
use kenn_model::{DefRecord, EdgeProperties, EdgeRecord, FileRecord, Kind, Language, SymbolRecord};
use kenn_store::api::WriteBatch;
use kenn_store::{open_writer, reader_from_writer, DbReader, DbWriter, WriterOptions};
use tempfile::TempDir;

const FILE: u32 = 1;
const DOC: u32 = 2;
const FLOW: u32 = 3;
const TOKENS: u32 = 4;

const DOC_ID: &str = "md:workspace/docs/auth.md";
const FLOW_ID: &str = "md:workspace/docs/auth.md#flow";
const TOKENS_ID: &str = "md:workspace/docs/auth.md#tokens";

fn md_symbol(id: u32, pub_id: &str, kind: Kind, name: &str, enclosing: u32) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: pub_id.into(),
        language: Language::Markdown,
        pkg_id: 0,
        kind,
        name: name.into(),
        enclosing_sym_id: enclosing,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

fn def(sym: u32, start: u32, end: u32) -> DefRecord {
    DefRecord {
        sym_id: sym,
        file_id: FILE,
        start_line: start,
        start_col: 0,
        end_line: end,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    }
}

/// A small markdown corpus: one document with two nested sections.
async fn build_corpus(dir: &Path) -> DbWriter {
    let writer = open_writer(dir, WriterOptions::default())
        .await
        .expect("open_writer");
    let batch = WriteBatch {
        packages: Vec::new(),
        files: vec![FileRecord {
            id: FILE,
            path: "docs/auth.md".into(),
            language: Language::Markdown,
            test: false,
            external: false,
            content_hash: 0,
        }],
        symbols: vec![
            md_symbol(DOC, DOC_ID, Kind::Document, "Auth", 0),
            md_symbol(FLOW, FLOW_ID, Kind::Section, "Flow", DOC),
            md_symbol(TOKENS, TOKENS_ID, Kind::Section, "Tokens", DOC),
        ],
        symbol_docs: Vec::new(),
        file_docs: Vec::new(),
        defs: vec![def(DOC, 1, 10), def(FLOW, 2, 5), def(TOKENS, 6, 10)],
        // child --defined_in--> parent drives list_in_scope on the document.
        edges: vec![
            EdgeRecord {
                src_id: FLOW,
                target_id: DOC,
                properties: EdgeProperties::DefinedIn,
            },
            EdgeRecord {
                src_id: TOKENS,
                target_id: DOC,
                properties: EdgeProperties::DefinedIn,
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
        snapshot_id: snapshot_id_from_timestamp("markdown-nav-test"),
        indexed_at: "markdown-nav-test".into(),
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

#[tokio::test(flavor = "multi_thread")]
async fn find_symbol_returns_md_section() {
    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let resp = find_symbol(
        &state,
        &FindSymbolArgs {
            name: "Flow".into(),
            kind: None,
            page_size: None,
            include_tests: None,
            include_external: None,
        },
    )
    .await
    .expect("find_symbol");
    let hit = resp
        .items
        .iter()
        .find(|r| r.base.id == FLOW_ID)
        .expect("flow section in results");
    assert_eq!(hit.base.kind, Kind::Section);
    assert_eq!(hit.base.language, Language::Markdown);
    assert_eq!(hit.match_kind, "exact");
}

#[tokio::test(flavor = "multi_thread")]
async fn search_symbols_returns_md_section() {
    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let resp = search_symbols(
        &state,
        &SearchSymbolsArgs {
            query: "tokens".into(),
            filters: None,
            pagination: Some(Pagination {
                page_size: Some(10),
                cursor: None,
            }),
        },
    )
    .await
    .expect("search_symbols");
    let found = resp.items.iter().any(|h| match h {
        SearchHitRef::Symbol(s) => s.id == TOKENS_ID,
        SearchHitRef::File(_) => false,
    });
    assert!(found, "tokens section should surface in search results");
}

#[tokio::test(flavor = "multi_thread")]
async fn list_in_scope_returns_document_sections() {
    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let resp = list_in_scope(
        &state,
        &ByIdArgs {
            id: DOC_ID.into(),
            filters: None,
            pagination: None,
        },
    )
    .await
    .expect("list_in_scope");
    let mut ids: Vec<&str> = resp.items.iter().map(|r| r.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, [FLOW_ID, TOKENS_ID]);
}

#[tokio::test(flavor = "multi_thread")]
async fn find_at_location_returns_enclosing_section() {
    let dir = TempDir::new().unwrap();
    let state = state_with_corpus(&dir).await;
    let resp = find_at_location(
        &state,
        &FindAtLocationArgs {
            file_path: "docs/auth.md".into(),
            line: 3, // inside the Flow section (lines 2–5)
            kind: None,
        },
    )
    .await
    .expect("find_at_location");
    // Tightest-enclosing first: the Flow section, not the whole document.
    assert_eq!(resp.items.first().map(|r| r.id.as_str()), Some(FLOW_ID));
    assert_eq!(resp.items[0].kind, Kind::Section);
}
