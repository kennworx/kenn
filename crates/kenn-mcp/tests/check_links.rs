//! Integration coverage for the `check_links` MCP tool (index-markdown 7.1) —
//! the read path for the `link_grade`/edge-kind columns. Seeds graded markdown
//! link edges and asserts the report lists the non-exact ones (and only those),
//! decoding dangling targets and rendering file targets from the files table.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kenn_mcp::snapshot_id_from_timestamp;
use kenn_mcp::state::LifecycleState;
use kenn_mcp::tools::{check_links, CheckLinksArgs, ServerState};
use kenn_model::{
    compose_short_id, DefRecord, EdgeProperties, EdgeRecord, FileRecord, Kind, Language, LinkGrade,
    SymbolRecord,
};
use kenn_store::api::WriteBatch;
use kenn_store::{open_writer, reader_from_writer, DbReader, DbWriter, WriterOptions};
use tempfile::TempDir;

const MD_FILE: u32 = 1;
const DOC: u32 = 2;
const SECTION: u32 = 3;
const OTHER_DOC: u32 = 4;
const STUB: u32 = 5;

fn md_symbol(id: u32, pub_id: &str, kind: Kind, external: bool) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: pub_id.into(),
        language: Language::Markdown,
        pkg_id: 0,
        kind,
        name: pub_id.rsplit(['/', '#']).next().unwrap_or(pub_id).into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external,
        test: false,
    }
}

fn links_to(src: u32, tgt: u32, grade: LinkGrade) -> EdgeRecord {
    EdgeRecord {
        src_id: src,
        target_id: tgt,
        properties: EdgeProperties::LinksTo {
            grade,
            relation: String::new(),
        },
    }
}

async fn build_corpus(dir: &Path) -> DbWriter {
    let writer = open_writer(dir, WriterOptions::default())
        .await
        .expect("open_writer");
    let code_file = compose_short_id(Language::Rust, 1);
    let batch = WriteBatch {
        packages: Vec::new(),
        files: vec![
            FileRecord {
                id: MD_FILE,
                path: "docs/guide.md".into(),
                language: Language::Markdown,
                test: false,
                external: false,
                content_hash: 0,
            },
            FileRecord {
                id: code_file,
                path: "src/order.rs".into(),
                language: Language::Rust,
                test: false,
                external: false,
                content_hash: 0,
            },
        ],
        symbols: vec![
            md_symbol(DOC, "md:workspace/docs/guide.md", Kind::Document, false),
            md_symbol(
                SECTION,
                "md:workspace/docs/guide.md#flow",
                Kind::Section,
                false,
            ),
            md_symbol(
                OTHER_DOC,
                "md:workspace/docs/other.md",
                Kind::Document,
                false,
            ),
            // dangling external stub: pub_id carries the written target.
            md_symbol(STUB, "md:@unresolved/ghost", Kind::Document, true),
        ],
        symbol_docs: Vec::new(),
        file_docs: Vec::new(),
        defs: vec![DefRecord {
            sym_id: SECTION,
            file_id: MD_FILE,
            start_line: 2,
            start_col: 0,
            end_line: 5,
            end_col: 0,
            body_start_line: 0,
            body_end_line: 0,
        }],
        edges: vec![
            links_to(SECTION, OTHER_DOC, LinkGrade::Drifted), // listed
            links_to(SECTION, STUB, LinkGrade::Dangling),     // listed (decoded)
            links_to(SECTION, DOC, LinkGrade::Exact),         // EXCLUDED (exact)
            EdgeRecord {
                src_id: SECTION,
                target_id: code_file,
                properties: EdgeProperties::LinksToFile {
                    grade: LinkGrade::Drifted,
                },
            }, // listed (file path)
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
        snapshot_id: snapshot_id_from_timestamp("check-links-test"),
        indexed_at: "check-links-test".into(),
        read: arc_swap::ArcSwap::from(Arc::new(kenn_mcp::state::ReaderBinding::new(reader, pin))),
        fallback_from_parent: false,
        reindex: None,
        run_meta: None,
    };
    state
}

#[tokio::test(flavor = "multi_thread")]
async fn check_links_lists_non_exact_links() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path()).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );

    let resp = check_links(&state, &CheckLinksArgs::default())
        .await
        .expect("check_links");

    // Exactly the three non-exact links; the exact one is excluded.
    assert_eq!(resp.total, 3);
    assert_eq!(resp.returned, 3);
    assert!(!resp.truncated);
    assert_eq!(resp.links.len(), 3);

    let by_kind_grade = |kind: &str, grade: &str| {
        resp.links
            .iter()
            .find(|l| l.kind == kind && l.grade == grade)
            .unwrap_or_else(|| panic!("missing {kind}/{grade} in {:?}", resp.links))
    };

    // Drifted md↔md link → target is the other document's id.
    let drifted = by_kind_grade("links_to", "drifted");
    assert_eq!(drifted.target, "md:workspace/docs/other.md");
    assert_eq!(drifted.location.as_deref(), Some("docs/guide.md#L2"));

    // Dangling link → the written target is decoded from the stub id.
    let dangling = by_kind_grade("links_to", "dangling");
    assert_eq!(dangling.target, "ghost");

    // File-target link → rendered as the code file path (not a symbol id).
    let file = by_kind_grade("links_to_file", "drifted");
    assert_eq!(file.target, "src/order.rs");

    // The exact link never appears.
    assert!(!resp.links.iter().any(|l| l.grade == "exact"));

    // grade filter narrows to just the requested grade.
    let only_dangling = check_links(
        &state,
        &CheckLinksArgs {
            grade: Some(vec!["dangling".into()]),
            limit: None,
        },
    )
    .await
    .expect("check_links dangling");
    assert_eq!(only_dangling.total, 1);
    assert!(only_dangling.links.iter().all(|l| l.grade == "dangling"));

    // limit caps the rows but `total` still reports the full count.
    let capped = check_links(
        &state,
        &CheckLinksArgs {
            grade: None,
            limit: Some(1),
        },
    )
    .await
    .expect("check_links capped");
    assert_eq!(capped.total, 3);
    assert_eq!(capped.returned, 1);
    assert!(capped.truncated);

    // an unknown grade name is a loud error, not a silent empty result.
    check_links(
        &state,
        &CheckLinksArgs {
            grade: Some(vec!["bogus".into()]),
            limit: None,
        },
    )
    .await
    .expect_err("unknown grade should error");
}
