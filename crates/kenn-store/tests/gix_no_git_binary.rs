//! gix-git-backend §3.2 / §3.3: staleness and worktree resolution need no
//! `git` binary on `PATH` — gix reads the repository in-process.
//!
//! This lives in its own integration-test binary with a single test so that
//! clearing the process-global `PATH` (after the git-subprocess fixture setup)
//! races with nothing. The runtime code path is already structurally free of
//! `Command::new("git")`; this asserts it end-to-end.

use std::path::Path;
use std::process::Command;

use kenn_store::staleness::{compute_staleness_key, StalenessKey};
use kenn_store::worktree::resolve_main_worktree;
use tempfile::TempDir;

fn git(args: &[&str], dir: &Path) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("spawn git")
        .success();
    assert!(ok, "git {args:?} failed");
}

#[test]
fn staleness_and_worktree_need_no_git_binary_on_path() {
    let dir = TempDir::new().unwrap();
    // Fixture setup uses the git binary (PATH still intact).
    git(&["init", "-q"], dir.path());
    git(&["config", "user.email", "t@example.invalid"], dir.path());
    git(&["config", "user.name", "t"], dir.path());
    git(&["config", "commit.gpgsign", "false"], dir.path());
    std::fs::write(dir.path().join("a.rs"), b"fn main() {}").unwrap();
    git(&["add", "-A"], dir.path());
    git(&["commit", "-q", "-m", "init"], dir.path());

    // Remove git from PATH entirely: any lingering subprocess `git` call would
    // now fail to spawn. Safe here — this is the only test in the binary.
    std::env::remove_var("PATH");

    // The git-form staleness key still resolves in-process.
    let key = compute_staleness_key(dir.path(), 0);
    assert!(
        matches!(key, StalenessKey::Git { .. }),
        "expected the git form with no git binary on PATH, got {key:?}"
    );

    // Worktree resolution still resolves the main worktree in-process.
    let main = resolve_main_worktree(dir.path()).expect("main worktree resolves without git");
    assert_eq!(
        std::fs::canonicalize(&main).unwrap(),
        std::fs::canonicalize(dir.path()).unwrap(),
    );
}
