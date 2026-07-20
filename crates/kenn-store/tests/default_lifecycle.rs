//! Full-cycle lifecycle test for the storage backend.
//!
//! Exercises the complete `Store::begin_indexing` → write corpus →
//! `IndexingHandle::publish` → `open_reader` flow against a fresh
//! tempdir, and confirms the published snapshot is readable through the
//! public factory.

use kenn_model::{
    EdgeProperties, EdgeRecord, FileRecord, Kind, Language, PackageRecord, SymbolDocsRecord,
    SymbolRecord,
};
use kenn_store::api::{Reader, WriteBatch};
use kenn_store::{begin_indexing, open_reader, open_writer, Store, WriterOptions};
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread")]
async fn diy_full_publish_cycle_then_open_reader() {
    let workspace = TempDir::new().unwrap();
    let store = Store::open_default(workspace.path()).expect("store");
    let handle = begin_indexing(&store).expect("begin_indexing");
    let run_dir = handle.run_dir().to_path_buf();

    // Write a small corpus through the DbWriter via the public factory.
    {
        let writer = open_writer(&run_dir, WriterOptions::default())
            .await
            .expect("open_writer");
        let batch = WriteBatch {
            packages: vec![PackageRecord {
                id: 1,
                name: "default-cycle".into(),
                version: "0.0.0".into(),
                manager: "cargo".into(),
                external: false,
            }],
            files: vec![FileRecord {
                id: 1,
                path: "src/lib.rs".into(),
                language: Language::Rust,
                test: false,
                external: false,
                content_hash: 0,
            }],
            symbols: vec![SymbolRecord {
                id: 10,
                pub_id: "rs:Foo".into(),
                language: Language::Rust,
                pkg_id: 1,
                kind: Kind::Class,
                name: "Foo".into(),
                enclosing_sym_id: 0,
                partial: false,
                nargs: 0,
                targs: 0,
                external: false,
                test: false,
            }],
            symbol_docs: vec![SymbolDocsRecord {
                sym_id: 10,
                sig: "class Foo".into(),
                doc: "the foo".into(),
            }],
            file_docs: vec![],
            defs: vec![],
            edges: vec![EdgeRecord {
                src_id: 10,
                target_id: 10,
                properties: EdgeProperties::Calls,
            }],
        };
        writer.write_batch(&batch).await.expect("write_batch");
        writer.finalize().await.expect("finalize");
        // writer drops here.
    }

    // Write a meta.json with a backend marker — the indexer normally
    // does this. For the integration test we mirror its shape.
    let meta = serde_json::json!({
        "timestamp": "2026-05-07T00:00:00Z",
        "status": "success",
        "backend": kenn_store::ACTIVE_BACKEND,
        "schema_version": kenn_store::STORE_SCHEMA_VERSION,
        "documents": 1u64,
        "symbols": 1u64,
        "definitions": 0u64,
        "edges": 1u64,
    });
    std::fs::write(
        run_dir.join("meta.json"),
        serde_json::to_vec_pretty(&meta).unwrap(),
    )
    .unwrap();

    // Publish — fsync run dir + atomic symlink flip (no rename under
    // D1; the run dir IS the published dir).
    let snapshot = handle.publish().expect("publish");
    assert!(snapshot.is_dir(), "published run dir exists");
    // The code graph + search store are SQLite databases.
    assert!(
        snapshot.join("code.db").is_file(),
        "code-graph SQLite db present"
    );
    assert!(
        snapshot.join("vector.db").is_file(),
        "knowledge SQLite db present"
    );

    // The `live` symlink should resolve to the published snapshot.
    let live = store.live_target().expect("live target");
    assert_eq!(live, snapshot, "live points at new snapshot");

    // Open a reader through the public factory and round-trip a fetch.
    let reader = open_reader(&snapshot).await.expect("open_reader");
    let row = reader
        .fetch_symbol("rust", "rs:Foo")
        .await
        .expect("fetch ok")
        .expect("Foo present");
    assert_eq!(row.id, 10);
    assert_eq!(row.name, "Foo");
}

/// `check_schema_version` pins the contract end-to-end:
/// - matching version → `Ok(persisted)`
/// - mismatched version → `SchemaMismatch` with both numbers
/// - field absent but `meta.json` present → treated as v1, mismatch
/// - `meta.json` absent entirely → bypass (raw fixture path)
#[test]
fn check_schema_version_enforces_strict_equality_only_when_meta_present() {
    use kenn_store::api::DbError;
    use kenn_store::{check_schema_version, STORE_SCHEMA_VERSION};

    let dir = TempDir::new().unwrap();

    // (a) No meta.json at all → bypass.
    let snap_a = dir.path().join("no_meta");
    std::fs::create_dir_all(&snap_a).unwrap();
    assert_eq!(
        check_schema_version(&snap_a).unwrap(),
        STORE_SCHEMA_VERSION,
        "no meta.json must bypass the check"
    );

    // (b) Meta with matching version → Ok.
    let snap_b = dir.path().join("match");
    std::fs::create_dir_all(&snap_b).unwrap();
    let meta = serde_json::json!({ "schema_version": STORE_SCHEMA_VERSION });
    std::fs::write(snap_b.join("meta.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
    assert_eq!(check_schema_version(&snap_b).unwrap(), STORE_SCHEMA_VERSION);

    // (c) Meta without `schema_version` field → defaults to v1 → mismatch
    //     (binaries with STORE_SCHEMA_VERSION >= 2 reject it).
    let snap_c = dir.path().join("missing_field");
    std::fs::create_dir_all(&snap_c).unwrap();
    std::fs::write(snap_c.join("meta.json"), br#"{"status":"success"}"#).unwrap();
    match check_schema_version(&snap_c) {
        Err(DbError::SchemaMismatch {
            persisted,
            expected,
        }) => {
            assert_eq!(persisted, 1);
            assert_eq!(expected, STORE_SCHEMA_VERSION);
        }
        other => panic!("expected SchemaMismatch, got {other:?}"),
    }

    // (d) Meta with explicit older version → mismatch with the persisted number.
    let snap_d = dir.path().join("older");
    std::fs::create_dir_all(&snap_d).unwrap();
    let older = serde_json::json!({ "schema_version": STORE_SCHEMA_VERSION - 1 });
    std::fs::write(
        snap_d.join("meta.json"),
        serde_json::to_vec(&older).unwrap(),
    )
    .unwrap();
    match check_schema_version(&snap_d) {
        Err(DbError::SchemaMismatch {
            persisted,
            expected,
        }) => {
            assert_eq!(persisted, STORE_SCHEMA_VERSION - 1);
            assert_eq!(expected, STORE_SCHEMA_VERSION);
        }
        other => panic!("expected SchemaMismatch, got {other:?}"),
    }
}
