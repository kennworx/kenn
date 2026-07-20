//! Regression for `index-producer-parity`: markdown must be indexed through the
//! workflow / MCP `index_workspace` path, not only the CLI.
//!
//! The producer set was configured in two drifted functions — `build_driver`
//! (CLI) and `configure_runner` (workflow/MCP) — and the workflow copy was
//! missing the `with_markdown` branch, so an MCP-triggered index silently
//! skipped markdown. They are now a single `configure_runner`. This test drives
//! the workflow path over a markdown-only repo: before the fix nothing was
//! indexed (0 nodes); after, the markdown file and its heading are.

use kenn_config::Config;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread")]
async fn markdown_is_indexed_via_index_workspace() {
    let dir = TempDir::new().unwrap();
    // The only content is one markdown file, and only markdown is enabled — so
    // any indexed node must have come from the markdown producer.
    std::fs::write(
        dir.path().join("guide.md"),
        "# Widget Authentication\n\nHow the widget authenticates against the gateway.\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("kenn.toml"),
        "[language.markdown]\nenabled = true\n",
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

    // Before the fix the workflow path had no markdown producer, so the file was
    // never discovered and both counts were 0.
    assert!(
        outcome.counts.documents + outcome.counts.symbols > 0,
        "markdown must be indexed via the workflow/MCP path; counts = {:?}",
        outcome.counts,
    );
}
