//! A query answered with no server anywhere in the picture.
//!
//! This is the test the `split-query-from-mcp` change exists to make possible,
//! and the one that keeps it honest. There is no `ServerState` here, no
//! lifecycle driven to `Ready`, no MCP peer, no watcher — a writer, a reader, a
//! [`QueryCtx`] assembled by hand, and an answer. If this file ever needs to
//! import `kenn-mcp` to compile, the layering has regressed.
//!
//! The coverage of each query's *behaviour* lives in `kenn-mcp/tests/`, driven
//! through the composed server the way production drives it. What is asserted
//! here is the shape of the seam itself.

use kenn_model::{FileRecord, Kind, Language, PackageRecord, SymbolRecord};
use kenn_query::{find_symbol, snapshot_id_from_timestamp, FindSymbolArgs, QueryCaches, QueryCtx};
use kenn_store::api::WriteBatch;
use kenn_store::{open_writer, reader_from_writer, WriterOptions};
use tempfile::TempDir;

fn sym(id: u32, pub_id: &str, name: &str) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: pub_id.into(),
        language: Language::Rust,
        pkg_id: 1,
        kind: Kind::Function,
        name: name.into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_query_answers_from_a_hand_built_context() {
    let dir = TempDir::new().expect("tempdir");

    let writer = open_writer(dir.path(), WriterOptions::default())
        .await
        .expect("open_writer");
    writer
        .write_batch(&WriteBatch {
            packages: vec![PackageRecord {
                id: 1,
                name: "standalone".into(),
                version: "0.0.0".into(),
                manager: "cargo".into(),
                external: false,
            }],
            files: vec![FileRecord {
                id: 1,
                path: "src/orders.rs".into(),
                language: Language::Rust,
                test: false,
                external: false,
                content_hash: 0,
            }],
            // Two symbols, not one. `find_symbol`'s last tier is n-gram fuzzy,
            // so against a single-symbol corpus it returns that symbol for any
            // input at all — a one-symbol fixture makes the name assertion
            // vacuous, and a mutation that queries a name the corpus does not
            // contain still passes. Found by mutation, per CLAUDE.md §9.
            symbols: vec![
                sym(1, "rs:orders::submit", "submit"),
                sym(2, "rs:orders::cancel", "cancel"),
            ],
            ..WriteBatch::default()
        })
        .await
        .expect("write_batch");
    writer.finalize().await.expect("finalize");

    let reader = reader_from_writer(&writer).await.expect("reader");
    let read = reader.connect().expect("connect");

    // Everything a read needs, and nothing else. The host normally fills these
    // from its own state; here they are literals, which is the point.
    let config = kenn_config::Config::default();
    let findings = tokio::sync::RwLock::new(None);
    let caches = QueryCaches::new();
    let ctx = QueryCtx {
        read: &read,
        indexed_at: "standalone-test",
        snapshot_id: snapshot_id_from_timestamp("standalone-test"),
        source_root: dir.path().to_path_buf(),
        config: &config,
        config_present: true,
        embed_stage: kenn_query::EmbedStage::Disabled,
        findings: &findings,
        caches: &caches,
    };

    let resp = find_symbol(
        &ctx,
        &FindSymbolArgs {
            name: "submit".into(),
            kind: None,
            page_size: None,
            include_tests: None,
            include_external: None,
        },
    )
    .await
    .expect("find_symbol");

    assert_eq!(resp.items.len(), 1, "one match: {:?}", resp.items);
    assert_eq!(resp.items[0].base.id, "rs:orders::submit");
    // The tier that matched is part of the answer: an exact-name hit must not
    // be reported as a fuzzy one, and a fuzzy fallback must not masquerade as
    // exact.
    assert_eq!(resp.items[0].match_kind, "exact");
}
