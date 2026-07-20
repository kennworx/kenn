//! When the snapshot has no aggregate-graph tables (the pre-Phase-2
//! shape), `kenn_analyze::projection::load_from_reader` returns an
//! empty graph. Index-time analysis and the `kenn visualize` command
//! both rely on this invariant to bail with the
//! `kenn index --force` hint.

use kenn_model::{
    DefRecord, EdgeProperties, EdgeRecord, FileRecord, Kind, Language, PackageRecord, SymbolRecord,
};
use kenn_store::api::WriteBatch;
use kenn_store::{open_reader, open_writer, WriterOptions};
use tempfile::TempDir;

#[tokio::test(flavor = "current_thread")]
async fn analyze_errors_when_aggregate_tables_missing() {
    let dir = TempDir::new().unwrap();
    let snapshot = dir.path().join("snapshot");

    // Write a tiny corpus through DbWriter, but DO NOT run the
    // aggregation step — simulates a snapshot built by a pre-Phase-2
    // binary. `analyze` MUST refuse to run.
    let writer = open_writer(&snapshot, WriterOptions::default())
        .await
        .unwrap();
    writer
        .write_batch(&WriteBatch {
            files: vec![FileRecord {
                id: 10,
                path: "src/lib.rs".into(),
                language: Language::Rust,
                test: false,
                external: false,
                content_hash: 0,
            }],
            packages: vec![PackageRecord {
                id: 1,
                name: "demo".into(),
                version: "0".into(),
                manager: "cargo".into(),
                external: false,
            }],
            symbols: vec![
                symbol(100, Kind::Class, "Foo", 0, 1),
                symbol(101, Kind::Method, "foo_m", 100, 1),
            ],
            symbol_docs: vec![],
            file_docs: vec![],
            defs: vec![def(100, 10), def(101, 10)],
            edges: vec![EdgeRecord {
                src_id: 101,
                target_id: 100,
                properties: EdgeProperties::Calls,
            }],
        })
        .await
        .unwrap();
    writer.finalize().await.unwrap();
    drop(writer);

    let reader = open_reader(&snapshot).await.unwrap();
    let graph = kenn_analyze::projection::load_from_reader(&reader)
        .await
        .expect("load_from_reader should not error on a snapshot without aggregate tables");
    assert!(
        graph.is_empty(),
        "snapshot lacking aggregate tables must produce an empty graph",
    );
}

fn symbol(id: u32, kind: Kind, name: &str, enclosing: u32, pkg_id: u32) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: format!("rs:{name}"),
        language: Language::Rust,
        pkg_id,
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

fn def(sym_id: u32, file_id: u32) -> DefRecord {
    DefRecord {
        sym_id,
        file_id,
        start_line: 1,
        start_col: 0,
        end_line: 1,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    }
}
