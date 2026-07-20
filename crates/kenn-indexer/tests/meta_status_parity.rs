//! Regression: the workflow / MCP `index_workspace` path must record the HONEST
//! aggregate status in `meta.json`, not a hardcoded `"success"`.
//!
//! Previously `write_run_meta` wrote `status: "success"` unconditionally, so a
//! partial run (one driver fails, another succeeds) published a snapshot whose
//! meta.json claimed success with no failed projects — `kenn status` showed
//! green on a partially-failed MCP-triggered index. This drives a partial run:
//! markdown succeeds in-process, while the C# unit fails because its configured
//! project (`nope.sln`) can't be resolved. It then asserts status is `partial`.

use kenn_config::Config;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread")]
async fn partial_run_records_partial_status_not_success() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("guide.md"), "# Guide\n\nsome prose\n").unwrap();
    std::fs::write(
        dir.path().join("kenn.toml"),
        "[language.markdown]\nenabled = true\n\n\
         [language.csharp]\nenabled = true\nprojects = [\"nope.sln\"]\n",
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
    .expect("a partial run (one unit failed, one succeeded) still publishes");

    let meta: serde_json::Value = serde_json::from_slice(
        &std::fs::read(outcome.snapshot_path.join("meta.json")).expect("meta.json exists"),
    )
    .expect("meta.json parses");

    assert_eq!(
        meta["status"], "partial",
        "a partial run must record status=partial, not a hardcoded success; meta = {meta}",
    );
    // report.json is now written on the workflow/MCP path too (was CLI-only).
    assert!(
        outcome.snapshot_path.join("report.json").exists(),
        "report.json must be written on the workflow/MCP path",
    );
}
