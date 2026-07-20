//! Integration coverage for the `check_css` MCP tool (index-css Group 9) — the
//! read path for the stylesheet graph. Seeds class/module nodes with
//! `uses_css_class` / `imports` / `defined_in` edges and asserts the report
//! lists orphan classes and dead stylesheets (and only those), gating
//! orphan-class on whether class-usage mining ran.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kenn_mcp::snapshot_id_from_timestamp;
use kenn_mcp::state::LifecycleState;
use kenn_mcp::tools::{check_css, CheckCssArgs, ServerState};
use kenn_model::{
    DefRecord, EdgeProperties, EdgeRecord, FileRecord, Kind, Language, LinkGrade, SymbolRecord,
};
use kenn_store::api::WriteBatch;
use kenn_store::{open_writer, reader_from_writer, DbReader, DbWriter, WriterOptions};
use tempfile::TempDir;

// Node ids (raw, distinct).
const CODE_FILE: u32 = 1;
const APP: u32 = 2;
const F_A: u32 = 3;
const MOD_A: u32 = 4;
const C_USED: u32 = 5;
const C_ORPHAN: u32 = 6;
const F_LIVE: u32 = 7;
const MOD_LIVE: u32 = 8;
const C_LIVE: u32 = 9;
const F_DEAD: u32 = 10;
const MOD_DEAD: u32 = 11;
const C_DEAD: u32 = 12;

fn css_symbol(id: u32, pub_id: &str, kind: Kind) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: pub_id.into(),
        language: Language::Css,
        pkg_id: 0,
        kind,
        name: pub_id
            .rsplit([':', '#', '/'])
            .next()
            .unwrap_or(pub_id)
            .into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

fn def(sym: u32, file: u32) -> DefRecord {
    DefRecord {
        sym_id: sym,
        file_id: file,
        start_line: 1,
        start_col: 0,
        end_line: 1,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    }
}

fn defined_in(class: u32, module: u32) -> EdgeRecord {
    EdgeRecord {
        src_id: class,
        target_id: module,
        properties: EdgeProperties::DefinedIn,
    }
}

fn css_file(id: u32, path: &str) -> FileRecord {
    FileRecord {
        id,
        path: path.into(),
        language: Language::Css,
        test: false,
        external: false,
        content_hash: 0,
    }
}

/// Build a stylesheet corpus. `mining` adds a `uses_css_class` edge (App → .used)
/// so orphan-class detection is enabled.
async fn build_corpus(dir: &Path, mining: bool) -> DbWriter {
    let writer = open_writer(dir, WriterOptions::default())
        .await
        .expect("open_writer");
    let mut edges = vec![
        defined_in(C_USED, MOD_A),
        defined_in(C_ORPHAN, MOD_A),
        defined_in(C_LIVE, MOD_LIVE),
        defined_in(C_DEAD, MOD_DEAD),
        // a.css @imports live.css → live.css is not an orphan stylesheet.
        EdgeRecord {
            src_id: MOD_A,
            target_id: MOD_LIVE,
            properties: EdgeProperties::Imports {
                kind: kenn_model::ImportKind::Explicit,
            },
        },
    ];
    if mining {
        edges.push(EdgeRecord {
            src_id: APP,
            target_id: C_USED,
            properties: EdgeProperties::UsesCssClass {
                grade: LinkGrade::Exact,
            },
        });
    }
    let batch = WriteBatch {
        packages: Vec::new(),
        files: vec![
            FileRecord {
                id: CODE_FILE,
                path: "src/app.ts".into(),
                language: Language::TypeScript,
                test: false,
                external: false,
                content_hash: 0,
            },
            css_file(F_A, "a.css"),
            css_file(F_LIVE, "live.css"),
            css_file(F_DEAD, "dead.css"),
        ],
        symbols: vec![
            SymbolRecord {
                id: APP,
                pub_id: "ts:app.App".into(),
                language: Language::TypeScript,
                pkg_id: 0,
                kind: Kind::Function,
                name: "App".into(),
                enclosing_sym_id: 0,
                partial: false,
                nargs: 0,
                targs: 0,
                external: false,
                test: false,
            },
            css_symbol(MOD_A, "css:a.css", Kind::Module),
            css_symbol(C_USED, "css:a.css#class:used", Kind::CssClass),
            css_symbol(C_ORPHAN, "css:a.css#class:orphan", Kind::CssClass),
            css_symbol(MOD_LIVE, "css:live.css", Kind::Module),
            css_symbol(C_LIVE, "css:live.css#class:live", Kind::CssClass),
            css_symbol(MOD_DEAD, "css:dead.css", Kind::Module),
            css_symbol(C_DEAD, "css:dead.css#class:dead", Kind::CssClass),
        ],
        symbol_docs: Vec::new(),
        file_docs: Vec::new(),
        defs: vec![
            def(APP, CODE_FILE),
            def(MOD_A, F_A),
            def(C_USED, F_A),
            def(C_ORPHAN, F_A),
            def(MOD_LIVE, F_LIVE),
            def(C_LIVE, F_LIVE),
            def(MOD_DEAD, F_DEAD),
            def(C_DEAD, F_DEAD),
        ],
        edges,
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
        snapshot_id: snapshot_id_from_timestamp("check-css-test"),
        indexed_at: "check-css-test".into(),
        read: arc_swap::ArcSwap::from(Arc::new(kenn_mcp::state::ReaderBinding::new(reader, pin))),
        fallback_from_parent: false,
        reindex: None,
        run_meta: None,
    };
    state
}

#[tokio::test(flavor = "multi_thread")]
async fn check_css_lists_orphans_with_mining_on() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path(), true).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );

    let resp = check_css(&state, &CheckCssArgs::default())
        .await
        .expect("check_css");

    // Orphan classes: .orphan, .live, .dead (all unused). Not .used.
    // Orphan stylesheets: dead.css (no inbound imports, no used selector).
    assert_eq!(resp.note, None);
    let classes: Vec<&str> = resp
        .findings
        .iter()
        .filter(|f| f.category == "orphan_class")
        .map(|f| f.pub_id.as_str())
        .collect();
    assert!(classes.contains(&"css:a.css#class:orphan"));
    assert!(classes.contains(&"css:live.css#class:live"));
    assert!(classes.contains(&"css:dead.css#class:dead"));
    assert!(!classes.contains(&"css:a.css#class:used"));

    let sheets: Vec<&str> = resp
        .findings
        .iter()
        .filter(|f| f.category == "orphan_stylesheet")
        .map(|f| f.pub_id.as_str())
        .collect();
    assert_eq!(sheets, ["css:dead.css"]);
    assert_eq!(resp.total, 4);

    // category filter narrows to just stylesheets.
    let only_sheets = check_css(
        &state,
        &CheckCssArgs {
            category: Some(vec!["orphan_stylesheet".into()]),
            limit: None,
        },
    )
    .await
    .expect("check_css sheets");
    assert_eq!(only_sheets.total, 1);
    assert!(only_sheets
        .findings
        .iter()
        .all(|f| f.category == "orphan_stylesheet"));

    // limit caps rows but total reports the full count.
    let capped = check_css(
        &state,
        &CheckCssArgs {
            category: None,
            limit: Some(1),
        },
    )
    .await
    .expect("check_css capped");
    assert_eq!(capped.total, 4);
    assert_eq!(capped.returned, 1);
    assert!(capped.truncated);

    // an unknown category is a loud error.
    check_css(
        &state,
        &CheckCssArgs {
            category: Some(vec!["bogus".into()]),
            limit: None,
        },
    )
    .await
    .expect_err("unknown category should error");
}

#[tokio::test(flavor = "multi_thread")]
async fn check_css_skips_orphan_class_when_mining_off() {
    let dir = TempDir::new().unwrap();
    let writer = build_corpus(dir.path(), false).await;
    let state = ready_state(
        dir.path(),
        reader_from_writer(&writer).await.expect("reader"),
    );

    let resp = check_css(&state, &CheckCssArgs::default())
        .await
        .expect("check_css");

    // No usage edges → orphan_class skipped with a note; stylesheets still work.
    assert!(resp.note.is_some());
    assert!(resp.findings.iter().all(|f| f.category != "orphan_class"));
    assert!(resp
        .findings
        .iter()
        .any(|f| f.pub_id == "css:dead.css" && f.category == "orphan_stylesheet"));
}
