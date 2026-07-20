//! Integration tests for `store-layout`: config-driven derived-store
//! placement, and staleness-keyed snapshot resolution across two
//! branches sharing one relocated derived root.

use std::path::{Path, PathBuf};
use std::process::Command;

use kenn_config::Config;
use kenn_store::{begin_indexing, decide_startup_state, gc, Layout, StartupDecision, Store};
use tempfile::TempDir;

/// Initialize a git repo at `dir` with one commit, so
/// `compute_staleness_key` yields a `Some` HEAD. `marker` makes the
/// committed tree unique, so distinct repos get distinct HEAD SHAs —
/// standing in for distinct branches.
fn git_repo(dir: &Path, marker: &str) {
    for args in [
        &["init", "-q"][..],
        &["config", "user.email", "t@t.invalid"],
        &["config", "user.name", "t"],
        &["config", "commit.gpgsign", "false"],
    ] {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git command");
    }
    std::fs::write(dir.join("a.rs"), format!("fn main() {{ /* {marker} */ }}"))
        .expect("write a.rs");
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir)
        .status()
        .expect("git add");
    Command::new("git")
        .args(["commit", "-q", "-m", "init"])
        .current_dir(dir)
        .status()
        .expect("git commit");
}

/// A config whose `[layout] derived_root` points at `derived`.
fn config_with_derived_root(derived: &Path) -> Config {
    Config::from_toml(&format!(
        "[layout]\nderived_root = {:?}\n",
        derived.to_str().expect("utf-8 derived path")
    ))
    .expect("parse config")
}

/// Publish one (empty) run through the lifecycle, stamping
/// `meta.json` carrying `key`. Under D1 the `meta.json` must be
/// written BEFORE `publish` (it's what marks the run complete).
fn publish_with_key(store: &Store, key: &kenn_store::StalenessKey) -> PathBuf {
    let handle = begin_indexing(store).expect("begin_indexing");
    let meta = serde_json::json!({
        "timestamp": "x", "status": "success",
        "schema_version": kenn_store::STORE_SCHEMA_VERSION,
        "backend": kenn_store::ACTIVE_BACKEND,
        "documents": 0, "symbols": 0, "definitions": 0, "edges": 0,
        "staleness_key": key,
    });
    let bytes = serde_json::to_vec(&meta).expect("serialize meta");
    std::fs::write(handle.run_dir().join("meta.json"), bytes).expect("write meta.json");
    handle.publish().expect("publish")
}

#[test]
fn default_layout_writes_the_derived_store_in_repo() {
    let repo = TempDir::new().unwrap();
    let layout = Layout::resolve(&Config::default(), repo.path()).unwrap();
    let store = Store::open(layout).unwrap();
    let handle = begin_indexing(&store).unwrap();
    std::fs::write(
        handle.run_dir().join("meta.json"),
        b"{\"status\":\"success\"}",
    )
    .unwrap();
    let snap = handle.publish().unwrap();

    // The published run lands under the in-repo `.kenn/local/runs/`.
    assert!(snap.starts_with(repo.path().join(".kenn").join("local")));
    assert!(repo
        .path()
        .join(".kenn")
        .join("local")
        .join("runs")
        .is_dir());
}

#[test]
fn configured_derived_root_relocates_every_derived_artifact() {
    let repo = TempDir::new().unwrap();
    let elsewhere = TempDir::new().unwrap();
    let derived = elsewhere.path().join("store");
    let layout = Layout::resolve(&config_with_derived_root(&derived), repo.path()).unwrap();
    let store = Store::open(layout.clone()).unwrap();
    let handle = begin_indexing(&store).unwrap();
    std::fs::write(
        handle.run_dir().join("meta.json"),
        b"{\"status\":\"success\"}",
    )
    .unwrap();
    let snap = handle.publish().unwrap();

    // The published run and every derived path resolve under the
    // configured root — nothing derived is written in-repo.
    assert!(
        snap.starts_with(&derived),
        "run under the configured derived root"
    );
    assert!(layout.scip_path("rust").starts_with(&derived));
    assert!(
        !repo.path().join(".kenn").join("local").exists(),
        "no derived `.kenn/local` in the repo"
    );
    // The committed root stays in-repo regardless.
    assert_eq!(layout.committed_root(), repo.path().join(".kenn"));
}

#[test]
fn two_branches_share_one_derived_root_without_clobbering() {
    // A shared derived root, two repos standing in for two branches.
    let shared = TempDir::new().unwrap();
    let cfg = config_with_derived_root(shared.path());

    let branch_a = TempDir::new().unwrap();
    let branch_b = TempDir::new().unwrap();
    git_repo(branch_a.path(), "alpha");
    git_repo(branch_b.path(), "beta");

    let layout_a = Layout::resolve(&cfg, branch_a.path()).unwrap();
    let layout_b = Layout::resolve(&cfg, branch_b.path()).unwrap();
    assert_eq!(layout_a.derived_root(), layout_b.derived_root());

    let store_a = Store::open(layout_a.clone()).unwrap();
    let store_b = Store::open(layout_b.clone()).unwrap();
    let key_a = kenn_store::compute_staleness_key(branch_a.path(), 0);
    let key_b = kenn_store::compute_staleness_key(branch_b.path(), 0);

    let snap_a = publish_with_key(&store_a, &key_a);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let snap_b = publish_with_key(&store_b, &key_b);

    // Each branch resolves its own matching snapshot — the shared `live`
    // pointer (now at B) does not clobber A.
    match decide_startup_state(&store_a, branch_a.path(), true, 0) {
        StartupDecision::Skip { live } => assert_eq!(live, snap_a),
        StartupDecision::Reindex { reason } => {
            panic!("branch A: expected Skip(snap_a), got Reindex({reason})")
        }
    }
    match decide_startup_state(&store_b, branch_b.path(), true, 0) {
        StartupDecision::Skip { live } => assert_eq!(live, snap_b),
        StartupDecision::Reindex { reason } => {
            panic!("branch B: expected Skip(snap_b), got Reindex({reason})")
        }
    }

    // A third branch with no matching snapshot reindexes.
    let branch_c = TempDir::new().unwrap();
    git_repo(branch_c.path(), "gamma");
    let store_c = Store::open(Layout::resolve(&cfg, branch_c.path()).unwrap()).unwrap();
    assert!(matches!(
        decide_startup_state(&store_c, branch_c.path(), true, 0),
        StartupDecision::Reindex { .. }
    ));
}

#[test]
fn non_git_workspace_skips_when_unchanged_and_reindexes_after_an_edit() {
    // A non-git workspace flows through the same staleness-keyed
    // resolution (`decide_startup_state`) that the MCP server and
    // `kenn embed` both use — no special-casing.
    let workspace = TempDir::new().unwrap();
    std::fs::write(workspace.path().join("a.rs"), "fn main() {}").unwrap();
    let layout = Layout::resolve(&Config::default(), workspace.path()).unwrap();
    let store = Store::open(layout).unwrap();

    // Simulate an index run: publish a snapshot stamped with the
    // workspace's current tree-fingerprint key.
    let snap = publish_with_key(
        &store,
        &kenn_store::compute_staleness_key(workspace.path(), 0),
    );

    // Unchanged → the snapshot resolves and indexing is skipped.
    match decide_startup_state(&store, workspace.path(), true, 0) {
        StartupDecision::Skip { live } => assert_eq!(live, snap),
        StartupDecision::Reindex { reason } => panic!("expected Skip, got Reindex({reason})"),
    }

    // Edit a source file → the tree fingerprint changes → reindex.
    std::fs::write(workspace.path().join("a.rs"), "fn main() { /* edited */ }").unwrap();
    assert!(matches!(
        decide_startup_state(&store, workspace.path(), true, 0),
        StartupDecision::Reindex { .. }
    ));
}

#[test]
fn actively_used_branch_snapshot_survives_another_branch_reindex() {
    let shared = TempDir::new().unwrap();
    let cfg = config_with_derived_root(shared.path());
    let branch_a = TempDir::new().unwrap();
    let branch_b = TempDir::new().unwrap();
    git_repo(branch_a.path(), "alpha");
    git_repo(branch_b.path(), "beta");

    let store_a = Store::open(Layout::resolve(&cfg, branch_a.path()).unwrap()).unwrap();
    let store_b = Store::open(Layout::resolve(&cfg, branch_b.path()).unwrap()).unwrap();
    let key_a = kenn_store::compute_staleness_key(branch_a.path(), 0);

    let snap_a = publish_with_key(&store_a, &key_a);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let h_b = begin_indexing(&store_b).unwrap();
    std::fs::write(h_b.run_dir().join("meta.json"), b"{\"status\":\"success\"}").unwrap();
    let _snap_b1 = h_b.publish().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));

    // Branch A is used — resolution refreshes its access time.
    assert!(matches!(
        decide_startup_state(&store_a, branch_a.path(), true, 0),
        StartupDecision::Skip { .. }
    ));

    // Branch B reindexes; GC (sized for both branches) runs.
    let h_b2 = begin_indexing(&store_b).unwrap();
    std::fs::write(
        h_b2.run_dir().join("meta.json"),
        b"{\"status\":\"success\"}",
    )
    .unwrap();
    let _snap_b2 = h_b2.publish().unwrap();
    gc(&store_a, 2).unwrap();

    // A's snapshot survived — it is among the recently-accessed.
    assert!(
        snap_a.is_dir(),
        "the actively-used branch's snapshot is retained"
    );
}
