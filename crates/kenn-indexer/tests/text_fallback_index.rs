//! End-to-end for `text-fallback-index`: configured non-semantic text files
//! become searchable nodes through the workflow / MCP `index_workspace` path,
//! and the fallback is a no-op when disabled (the default).
//!
//! Scenario coverage mirrors the change's spec: 3.1 (a `.yaml` becomes chunk
//! nodes) and 3.3 (disabled → no behavior change). The double-index guard (3.2)
//! is exercised by the `discover` / `ingest` unit tests, which assert the
//! claimed-extension skip directly without needing a real language server.

use kenn_config::Config;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread")]
async fn text_file_is_indexed_via_index_workspace() {
    let dir = TempDir::new().unwrap();
    // The only content is one YAML file, and only the text fallback is enabled —
    // so any indexed node must have come from the text producer.
    std::fs::write(
        dir.path().join("config.yaml"),
        "service:\n  name: gateway\n  port: 8080\n  auth: token\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("kenn.toml"),
        "[language.text]\nenabled = true\ninclude = [\"**/*.yaml\"]\n",
    )
    .unwrap();

    let config = Config::load_from_path(&dir.path().join("kenn.toml")).unwrap();
    let outcome = kenn_indexer::index_workspace(
        &kenn_store::Layout::default_for(dir.path()),
        &config,
        |_ev| {},
        kenn_indexer::pipeline::no_op_hook(),
    )
    .await
    .expect("index_workspace should succeed");

    assert!(
        outcome.counts.documents + outcome.counts.symbols > 0,
        "text fallback must index the yaml via the workflow/MCP path; counts = {:?}",
        outcome.counts,
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn disabled_text_fallback_indexes_nothing() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("config.yaml"), "a: 1\nb: 2\n").unwrap();
    // No config at all: the fallback is disabled by default.
    std::fs::write(dir.path().join("kenn.toml"), "").unwrap();

    let config = Config::load_from_path(&dir.path().join("kenn.toml")).unwrap();
    let outcome = kenn_indexer::index_workspace(
        &kenn_store::Layout::default_for(dir.path()),
        &config,
        |_ev| {},
        kenn_indexer::pipeline::no_op_hook(),
    )
    .await
    .expect("index_workspace should succeed");

    assert_eq!(
        outcome.counts.documents + outcome.counts.symbols,
        0,
        "disabled fallback must index nothing; counts = {:?}",
        outcome.counts,
    );
}
