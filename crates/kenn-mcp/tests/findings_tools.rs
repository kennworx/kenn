//! Integration coverage for the knowledge-layer MCP tools — findings
//! read/write, the derivation DAG, and unified search — driven through
//! `ServerState` against an in-process corpus plus a live findings
//! store. Mirrors `symbol_search.rs`'s `Ready`-state setup.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kenn_mcp::snapshot_id_from_timestamp;
use kenn_mcp::state::LifecycleState;
use kenn_mcp::tools::{
    check_anchors, find_directives, find_predecessors, get_finding, merge_findings, record_anchor,
    search_findings, semantic_search, store_finding, CheckAnchorsArgs, FindDirectivesArgs,
    FindingDagArgs, GetFindingArgs, MergeFindingsArgs, RecordAnchorArgs, SearchFindingsArgs,
    SearchScope, SemanticSearchArgs, ServerState, StoreFindingArgs,
};
use kenn_model::{FileRecord, Kind, Language, PackageRecord, SymbolRecord};
use kenn_store::api::WriteBatch;
use kenn_store::{open_writer, reader_from_writer, DbReader, DbWriter, Layout, WriterOptions};
// `FindingsStore` is opened indirectly by `ServerState::open_findings`.
use tempfile::TempDir;

const PKG: u32 = 1;
const FILE: u32 = 1;

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
            name: "findings-test".into(),
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
        symbols: vec![sym(101, "OrderHandler"), sym(102, "OrderRepository")],
        symbol_docs: Vec::new(),
        file_docs: Vec::new(),
        defs: Vec::new(),
        edges: Vec::new(),
    };
    writer.write_batch(&batch).await.expect("write_batch");
    writer.finalize().await.expect("finalize");
    writer
}

/// Materialize an empty live findings run at `workspace` so the
/// runs-centric findings store has a `live/lance/findings/` mirror to
/// append to and read from. `store_finding` then appends into it; a
/// keyword search is findable on return (the sync-BM25 write path).
async fn publish_empty_findings_live(workspace: &Path) {
    let layout = Layout::default_for(workspace);
    kenn_store::Store::open(layout.clone()).expect("store");
    let run_dir = layout.run_dir("findings-test-run");
    std::fs::create_dir_all(&run_dir).expect("run dir");
    let lock = kenn_store::stage_findings_for_publish(&layout, &run_dir)
        .await
        .expect("stage findings");
    let live = layout.live_path();
    drop(std::fs::remove_file(&live));
    std::fs::write(&live, "runs/findings-test-run").expect("live pointer");
    drop(lock);
}

/// Build a `Ready` server over an in-process reader, with a live
/// findings store opened at `workspace`.
async fn ready_state(workspace: PathBuf, reader: DbReader) -> ServerState {
    let state = ServerState::new(&workspace);
    let snap_path = PathBuf::from("in-process");
    let store = kenn_store::Store::open_default(&workspace).expect("store");
    let pin = kenn_store::readers::register_reader(&store, &snap_path).expect("pin");
    *state.lifecycle.write().expect("lifecycle lock") = LifecycleState::Ready {
        snapshot_path: snap_path,
        snapshot_id: snapshot_id_from_timestamp("findings-tools-test"),
        indexed_at: "findings-tools-test".into(),
        read: arc_swap::ArcSwap::from(Arc::new(kenn_mcp::state::ReaderBinding::new(reader, pin))),
        fallback_from_parent: false,
        reindex: None,
        run_meta: None,
    };
    // Publish an empty live findings run so the findings store resolves a
    // `live/lance/findings/` mirror. We open the findings store directly
    // rather than via `bootstrap`: `bootstrap`'s `open_ready_if_live`
    // would try to open that findings-only run as a *code* snapshot,
    // which we don't want to clobber the in-process `Ready` reader above.
    publish_empty_findings_live(&workspace).await;
    state.open_findings().await;
    state
}

/// `store_finding` returns `{id, similar}` and the finding is then
/// retrievable by id via `get_finding`.
#[tokio::test(flavor = "multi_thread")]
async fn store_finding_returns_id_and_is_retrievable() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path().to_path_buf(),
        reader_from_writer(&writer).await.expect("reader"),
    )
    .await;

    let resp = store_finding(
        &state,
        &StoreFindingArgs {
            text: "the order handler retries twice".into(),
            parent_ids: None,
            tags: Some(vec!["gotcha".into()]),
            anchors: None,
        },
    )
    .await
    .expect("store_finding");
    assert!(resp.id.starts_with("fnd_"));
    assert!(resp.similar.is_empty(), "similar is reserved/empty for now");

    let got = get_finding(
        &state,
        &GetFindingArgs {
            id: resp.id.clone(),
        },
    )
    .await
    .expect("get_finding");
    assert!(got.found);
    let f = got.item.unwrap();
    assert_eq!(f.id, resp.id);
    assert_eq!(f.text, "the order handler retries twice");
    assert_eq!(f.tags, vec!["gotcha".to_string()]);
}

/// `merge_findings` records its input ids as the new finding's
/// `parent_ids`.
#[tokio::test(flavor = "multi_thread")]
async fn merge_findings_records_inputs_as_parents() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path().to_path_buf(),
        reader_from_writer(&writer).await.expect("reader"),
    )
    .await;

    let a = store_finding(
        &state,
        &StoreFindingArgs {
            text: "fact a".into(),
            parent_ids: None,
            tags: None,
            anchors: None,
        },
    )
    .await
    .expect("store a")
    .id;
    let b = store_finding(
        &state,
        &StoreFindingArgs {
            text: "fact b".into(),
            parent_ids: None,
            tags: None,
            anchors: None,
        },
    )
    .await
    .expect("store b")
    .id;

    let merged = merge_findings(
        &state,
        &MergeFindingsArgs {
            ids: vec![a.clone(), b.clone()],
            text: "synthesis of a and b".into(),
            tags: None,
        },
    )
    .await
    .expect("merge_findings");
    assert!(merged.found);
    let merged_id = merged.item.unwrap();

    let got = get_finding(&state, &GetFindingArgs { id: merged_id })
        .await
        .expect("get_finding")
        .item
        .unwrap();
    assert!(got.parent_ids.contains(&a));
    assert!(got.parent_ids.contains(&b));
}

/// `find_predecessors` traces a finding back to an originating
/// code-graph node.
#[tokio::test(flavor = "multi_thread")]
async fn find_predecessors_traces_to_code_node() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path().to_path_buf(),
        reader_from_writer(&writer).await.expect("reader"),
    )
    .await;

    // A code-node id in the unified space is `<lang>:<pub_id>`.
    let code_node = "csharp:cs:OrderHandler".to_string();
    let base = store_finding(
        &state,
        &StoreFindingArgs {
            text: "OrderHandler does retries".into(),
            parent_ids: Some(vec![code_node.clone()]),
            tags: None,
            anchors: None,
        },
    )
    .await
    .expect("store base")
    .id;
    let derived = store_finding(
        &state,
        &StoreFindingArgs {
            text: "derived from the base fact".into(),
            parent_ids: Some(vec![base.clone()]),
            tags: None,
            anchors: None,
        },
    )
    .await
    .expect("store derived")
    .id;

    let preds = find_predecessors(&state, &FindingDagArgs { id: derived })
        .await
        .expect("find_predecessors");
    assert!(preds.items.contains(&base), "reaches the parent finding");
    assert!(
        preds.items.contains(&code_node),
        "transitively reaches the originating code-graph node"
    );
}

/// `search_findings` returns a `ListResponse` of ranked hits, each
/// carrying a `stale` flag.
#[tokio::test(flavor = "multi_thread")]
async fn search_findings_returns_ranked_hits() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path().to_path_buf(),
        reader_from_writer(&writer).await.expect("reader"),
    )
    .await;

    store_finding(
        &state,
        &StoreFindingArgs {
            text: "the cache evicts on every write".into(),
            parent_ids: None,
            tags: None,
            anchors: None,
        },
    )
    .await
    .expect("store");

    let resp = search_findings(
        &state,
        &SearchFindingsArgs {
            query: "cache evicts".into(),
            pagination: None,
        },
    )
    .await
    .expect("search_findings");
    assert_eq!(resp.items.len(), 1);
    assert_eq!(
        resp.items[0].finding.text,
        "the cache evicts on every write"
    );
    // A finding with no code parents is never stale.
    assert!(!resp.items[0].finding.stale);
}

/// `search_findings` paginates a multi-page top-K window via the
/// `ResultCache`. With more matches than `page_size`, the first call
/// emits a cursor; walking it reproduces the single-shot order. The
/// cumulative count never exceeds `TOP_K_MATERIALIZE = 30` (the
/// server-side cap), regardless of how many findings actually match
/// the query.
#[tokio::test(flavor = "multi_thread")]
async fn search_findings_paginates_to_cap() {
    use kenn_mcp::Pagination;

    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path().to_path_buf(),
        reader_from_writer(&writer).await.expect("reader"),
    )
    .await;

    // Store 12 findings that all match the query "cache" — enough to
    // exceed the default page_size of 10 and force pagination.
    for i in 0..12 {
        store_finding(
            &state,
            &StoreFindingArgs {
                text: format!("the cache item number {i}"),
                parent_ids: None,
                tags: None,
                anchors: None,
            },
        )
        .await
        .expect("store");
    }

    // Single-shot at page_size=30 (the materialize cap) returns the
    // full top-K window in one page.
    let single = search_findings(
        &state,
        &SearchFindingsArgs {
            query: "cache".into(),
            pagination: Some(Pagination {
                page_size: Some(30),
                cursor: None,
            }),
        },
    )
    .await
    .expect("single-shot");
    assert!(single.next.is_none(), "page_size=30 must be single-shot");
    let single_ids: Vec<String> = single.items.iter().map(|h| h.finding.id.clone()).collect();
    assert!(
        single_ids.len() >= 10,
        "expected ≥10 matching findings, got {}",
        single_ids.len()
    );
    assert!(
        single_ids.len() <= 30,
        "cumulative must never exceed TOP_K_MATERIALIZE = 30, got {}",
        single_ids.len()
    );

    // Walking with page_size=4 must reach the same items in the same
    // order, demonstrating cache-backed continuation.
    let mut paged_ids: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let resp = search_findings(
            &state,
            &SearchFindingsArgs {
                query: "cache".into(),
                pagination: Some(Pagination {
                    page_size: Some(4),
                    cursor: cursor.clone(),
                }),
            },
        )
        .await
        .expect("page");
        paged_ids.extend(resp.items.iter().map(|h| h.finding.id.clone()));
        pages += 1;
        assert!(pages < 20, "pagination must terminate");
        match resp.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(
        paged_ids, single_ids,
        "paged walk must reproduce single-shot order"
    );
    assert!(
        pages >= 2,
        "with page_size=4 vs 10+ matches, must take ≥2 pages"
    );
}

/// `semantic_search` scoped to `both` ranks code symbols and findings
/// in separate groups.
#[tokio::test(flavor = "multi_thread")]
async fn semantic_search_returns_both_groups() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path().to_path_buf(),
        reader_from_writer(&writer).await.expect("reader"),
    )
    .await;

    store_finding(
        &state,
        &StoreFindingArgs {
            text: "the order pipeline batches writes".into(),
            parent_ids: None,
            tags: None,
            anchors: None,
        },
    )
    .await
    .expect("store");

    let resp = semantic_search(
        &state,
        &SemanticSearchArgs {
            query: "order".into(),
            scope: Some(SearchScope::Both),
            page_size: None,
            include_tests: None,
            include_external: None,
        },
    )
    .await
    .expect("semantic_search");
    assert!(!resp.code.is_empty(), "code group has matching symbols");
    assert!(
        !resp.findings.is_empty(),
        "findings group has matching findings"
    );

    // Scoping to findings only drops the code group.
    let findings_only = semantic_search(
        &state,
        &SemanticSearchArgs {
            query: "order".into(),
            scope: Some(SearchScope::Findings),
            page_size: None,
            include_tests: None,
            include_external: None,
        },
    )
    .await
    .expect("semantic_search findings");
    assert!(findings_only.code.is_empty());
    assert!(!findings_only.findings.is_empty());
}

/// The directive flow: `store_finding` with `anchors` creates an anchored
/// directive in one call; `find_directives` surfaces it by an anchored path
/// (and not by an unrelated one); `record_anchor` appends attach/rename events
/// and rejects an unknown op; `check_anchors` reports anchors whose paths do
/// not resolve on disk.
#[tokio::test(flavor = "multi_thread")]
async fn directive_flow_find_record_check() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path().to_path_buf(),
        reader_from_writer(&writer).await.expect("reader"),
    )
    .await;

    // Store a directive anchored to a file — created and anchored in one call.
    let resp = store_finding(
        &state,
        &StoreFindingArgs {
            text: "run the embedder in-foreground on macOS".into(),
            parent_ids: None,
            tags: Some(vec!["directive".into(), "polarity:dont".into()]),
            anchors: Some(vec!["src/Orders.cs".into()]),
        },
    )
    .await
    .expect("store_finding");
    let id = resp.id;

    // `find_directives` surfaces it by its anchored path (structural leg only).
    let found = find_directives(
        &state,
        &FindDirectivesArgs {
            paths: vec!["src/Orders.cs".into()],
            query: None,
        },
    )
    .await
    .expect("find_directives");
    assert!(
        found.items.iter().any(|r| r.finding.id == id),
        "directive is surfaced by its anchored path"
    );

    // An unrelated path does not surface it.
    let none = find_directives(
        &state,
        &FindDirectivesArgs {
            paths: vec!["src/Unrelated.cs".into()],
            query: None,
        },
    )
    .await
    .expect("find_directives");
    assert!(!none.items.iter().any(|r| r.finding.id == id));

    // `record_anchor`: attach (re-confirm), rename, and an invalid op.
    record_anchor(
        &state,
        &RecordAnchorArgs {
            finding_id: id.clone(),
            op: "attach".into(),
            anchor: Some("src/More.cs".into()),
            from: None,
            to: None,
        },
    )
    .await
    .expect("attach");
    record_anchor(
        &state,
        &RecordAnchorArgs {
            finding_id: id.clone(),
            op: "rename".into(),
            anchor: None,
            from: Some("src/More.cs".into()),
            to: Some("src/Renamed.cs".into()),
        },
    )
    .await
    .expect("rename");
    let bad = record_anchor(
        &state,
        &RecordAnchorArgs {
            finding_id: id.clone(),
            op: "bogus".into(),
            anchor: None,
            from: None,
            to: None,
        },
    )
    .await;
    assert!(bad.is_err(), "an unknown op is rejected");

    // An unknown finding id is rejected (no orphan anchor log is created).
    let orphan = record_anchor(
        &state,
        &RecordAnchorArgs {
            finding_id: "fnd_does-not-exist".into(),
            op: "attach".into(),
            anchor: Some("src/Orders.cs".into()),
            from: None,
            to: None,
        },
    )
    .await;
    assert!(orphan.is_err(), "an unknown finding id is rejected");

    // None of the anchored paths exist on disk → reported as broken.
    let report = check_anchors(&state, &CheckAnchorsArgs {})
        .await
        .expect("check_anchors");
    assert!(
        report.broken.iter().any(|b| b.finding_id == id),
        "missing anchor paths are reported broken"
    );
}

/// Content drift: a directive anchored to a real file that later changes content
/// surfaces as `drifted` (not broken) in both `find_directives` and
/// `check_anchors`; an unedited anchor is live.
#[tokio::test(flavor = "multi_thread")]
async fn directive_content_drift_detected() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path().to_path_buf(),
        reader_from_writer(&writer).await.expect("reader"),
    )
    .await;

    // A real file under the workspace, anchored at its current content.
    let file = dir.path().join("notes.md");
    std::fs::write(&file, "version one\n").unwrap();
    let id = store_finding(
        &state,
        &StoreFindingArgs {
            text: "keep notes.md in sync with the schema".into(),
            parent_ids: None,
            tags: Some(vec!["directive".into()]),
            anchors: Some(vec!["notes.md".into()]),
        },
    )
    .await
    .expect("store_finding")
    .id;

    // Unchanged file → not drifted.
    let before = find_directives(
        &state,
        &FindDirectivesArgs {
            paths: vec!["notes.md".into()],
            query: None,
        },
    )
    .await
    .expect("find_directives");
    let hit = before
        .items
        .iter()
        .find(|r| r.finding.id == id)
        .expect("surfaced");
    assert!(!hit.finding.drifted, "unedited anchor is live");

    // Edit the file → content sha no longer matches.
    std::fs::write(&file, "version two — changed\n").unwrap();

    let after = find_directives(
        &state,
        &FindDirectivesArgs {
            paths: vec!["notes.md".into()],
            query: None,
        },
    )
    .await
    .expect("find_directives");
    let hit = after
        .items
        .iter()
        .find(|r| r.finding.id == id)
        .expect("surfaced");
    assert!(hit.finding.drifted, "edited anchor drifts");

    // check_anchors reports it drifted, not broken (the file still exists).
    let report = check_anchors(&state, &CheckAnchorsArgs {})
        .await
        .expect("check_anchors");
    assert!(
        report.drifted.iter().any(|d| d.finding_id == id),
        "edited anchor is in the drifted bucket"
    );
    assert!(
        !report.broken.iter().any(|b| b.finding_id == id),
        "an existing-but-edited file is not broken"
    );
}
