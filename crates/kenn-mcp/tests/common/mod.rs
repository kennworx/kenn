//! Shared test fixtures for `kenn-mcp` integration tests.
//!
//! Used by `background_reindex.rs` and `watcher.rs`. The functions here
//! skip the real indexer pipeline (slow, needs language toolchains) and
//! publish empty-but-valid Lance snapshots so MCP-side behavior can be
//! tested without bringing in `kenn-dotnet`, `scip-typescript`, etc.
//!
//! Each test file declares `mod common;` to pull these in. The
//! `#[allow(dead_code)]` covers functions used by only some consumers
//! — Rust integration tests are independent crates so per-test usage
//! varies.

#![allow(
    dead_code,
    reason = "Each integration test file consumes a subset of these helpers; cross-file usage tracking would need a workspace-level analysis. The fixtures are shared by intent."
)]

use std::path::Path;
use std::sync::Arc;

use kenn_config::Config;
use kenn_mcp::state::{LifecycleState, ReaderBinding};
use kenn_mcp::tools::ServerState;
use kenn_model::{FileRecord, Kind, Language, PackageRecord, SymbolRecord};
use kenn_store::api::WriteBatch;
use kenn_store::{lifecycle, open_writer, Layout, Store, WriterOptions};

/// Construct an `Arc<ServerState>` for `workspace` with the default
/// `Config`.
pub fn make_state(workspace: &Path) -> Arc<ServerState> {
    make_state_with_config(workspace, Config::default())
}

/// Construct an `Arc<ServerState>` with a caller-supplied `Config`.
pub fn make_state_with_config(workspace: &Path, config: Config) -> Arc<ServerState> {
    let layout = Layout::default_for(workspace);
    Arc::new(ServerState::with_layout_and_config(layout, config))
}

/// Publish a minimal-but-valid snapshot through the real lifecycle path
/// (`begin_indexing` → write a single symbol → finalize → publish).
/// Returns the published snapshot directory.
pub async fn publish_snapshot(workspace: &Path) -> std::path::PathBuf {
    let store = Store::open_default(workspace).expect("store");
    let h = lifecycle::begin_indexing(&store).expect("begin_indexing");
    // Write a minimal symbol so the Lance datasets exist on disk;
    // `open_reader` requires the `symbols/_versions` marker.
    let writer = open_writer(h.run_dir(), WriterOptions::default())
        .await
        .expect("writer");
    writer
        .write_batch(&WriteBatch {
            packages: vec![PackageRecord {
                id: 1,
                name: "fixture".into(),
                version: "0.0.0".into(),
                manager: "test".into(),
                external: false,
            }],
            files: vec![FileRecord {
                id: 1,
                path: "fixture.rs".into(),
                language: Language::Rust,
                test: false,
                external: false,
                content_hash: 0,
            }],
            symbols: vec![SymbolRecord {
                id: 1,
                pub_id: "fixture::X".into(),
                language: Language::Rust,
                pkg_id: 1,
                kind: Kind::Class,
                name: "X".into(),
                enclosing_sym_id: 0,
                partial: false,
                nargs: 0,
                targs: 0,
                external: false,
                test: false,
            }],
            symbol_docs: Vec::new(),
            file_docs: Vec::new(),
            defs: Vec::new(),
            edges: Vec::new(),
        })
        .await
        .expect("write_batch");
    writer.finalize().await.expect("finalize");
    drop(writer);
    // KVS2 / D1 — publish refuses without `meta.json`. Include the
    // schema version so `kenn_store::open_reader`'s schema check
    // (store-schema-versioning) accepts the fixture.
    let meta = serde_json::json!({
        "status": "success",
        "schema_version": kenn_store::STORE_SCHEMA_VERSION,
    });
    std::fs::write(
        h.run_dir().join("meta.json"),
        serde_json::to_vec(&meta).expect("meta serde"),
    )
    .expect("meta");
    h.publish().expect("publish")
}

/// Open the (just-published) snapshot as a `Ready` lifecycle state.
/// Skips the real indexer pipeline.
pub async fn place_ready(state: &Arc<ServerState>, snapshot_path: &Path) {
    let store = Store::open(state.layout()).expect("store");
    let pin = kenn_store::readers::register_reader(&store, snapshot_path).expect("pin");
    let reader = kenn_store::open_reader(snapshot_path)
        .await
        .expect("reader");
    let snap_id = kenn_mcp::cursor::snapshot_id_from_timestamp(
        snapshot_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(""),
    );
    // Mirror production `open_binding`: parse the snapshot's run metadata
    // so `get_index_status` can report a degraded run.
    let run_meta = kenn_indexer::SnapshotMeta::read(snapshot_path).map(Box::new);
    let mut g = state.lifecycle.write().expect("lifecycle lock");
    *g = LifecycleState::Ready {
        snapshot_path: snapshot_path.to_path_buf(),
        snapshot_id: snap_id,
        indexed_at: snapshot_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string(),
        read: arc_swap::ArcSwap::from(Arc::new(ReaderBinding::new(reader, pin))),
        fallback_from_parent: false,
        reindex: None,
        run_meta,
    };
}

/// Get the served snapshot path, or panic if not `Ready`. Tests treat
/// an unexpected state as a test bug.
#[expect(
    clippy::panic,
    reason = "test fixture: an unexpected state is a test bug"
)]
pub fn served_snapshot(state: &ServerState) -> std::path::PathBuf {
    match &*state.lifecycle.read().expect("lifecycle lock") {
        LifecycleState::Ready { snapshot_path, .. } => snapshot_path.clone(),
        other => panic!("expected Ready, got {:?}", other.kind()),
    }
}
