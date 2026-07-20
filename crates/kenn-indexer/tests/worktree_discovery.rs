//! Tasks 3.6, 3.6a, 3.6b — git linked-worktree discovery + exclusion.
//! Requires `git` on PATH.

use std::path::Path;
use std::process::Command;

use kenn_indexer::{discover_other_worktrees, Workspace};

fn git(args: &[&str], cwd: &Path) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn init_repo(root: &Path) {
    git(&["init", "--quiet", "--initial-branch=main"], root);
    git(&["config", "user.email", "test@example.com"], root);
    git(&["config", "user.name", "Test"], root);
    std::fs::write(root.join("README.md"), "x").expect("write README");
    git(&["add", "."], root);
    git(&["commit", "--quiet", "-m", "init"], root);
}

#[test]
fn linked_worktree_at_nonconventional_path_is_discovered() {
    if !git_available() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let main_repo = dir.path().join("repo");
    let wt_path = dir.path().join("wt").join("feature-x");
    std::fs::create_dir_all(&main_repo).unwrap();
    std::fs::create_dir_all(wt_path.parent().unwrap()).unwrap();
    init_repo(&main_repo);
    git(
        &[
            "worktree",
            "add",
            "--quiet",
            wt_path.to_str().unwrap(),
            "-b",
            "feature-x",
        ],
        &main_repo,
    );

    let others = discover_other_worktrees(&main_repo);
    let canon_wt = wt_path.canonicalize().unwrap();
    assert!(
        others.contains(&canon_wt),
        "expected {canon_wt:?} in discovered worktrees, got: {others:?}"
    );
}

#[test]
fn workspace_root_that_is_a_linked_worktree_is_not_self_excluded() {
    // Worktree at an arbitrary path — discovery is git-driven, NOT path-name
    // driven. Any disk location works as long as `git worktree list` knows it.
    if !git_available() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let main_repo = dir.path().join("repo");
    let wt_path = dir.path().join("arbitrary-name").join("xyz");
    std::fs::create_dir_all(&main_repo).unwrap();
    std::fs::create_dir_all(wt_path.parent().unwrap()).unwrap();
    init_repo(&main_repo);
    git(
        &[
            "worktree",
            "add",
            "--quiet",
            wt_path.to_str().unwrap(),
            "-b",
            "feature-x",
        ],
        &main_repo,
    );

    // Discovery from the linked worktree's perspective. The main repo path
    // SHOULD appear (it's "other" relative to the worktree); the worktree
    // itself SHOULD NOT (the workspace root is never self-excluded).
    let others = discover_other_worktrees(&wt_path);
    let canon_wt = wt_path.canonicalize().unwrap();
    let canon_main = main_repo.canonicalize().unwrap();
    assert!(
        !others.contains(&canon_wt),
        "self should be excluded from discovery"
    );
    assert!(
        others.contains(&canon_main),
        "expected main repo path among discovered worktrees: {others:?}"
    );
}

#[test]
fn linked_worktree_path_is_excluded_from_canonicalize_regardless_of_dir_name() {
    // The exclude check is "absolute path is under a git-discovered worktree
    // directory" — there is no special-cased path name. Use a directory name
    // that has nothing to do with worktree convention to prove the point.
    if !git_available() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let main_repo = dir.path().join("repo");
    // Arbitrary nested directory under the main repo.
    let wt_path = main_repo
        .join("scratch")
        .join("branch-experiments")
        .join("xyz");
    std::fs::create_dir_all(&main_repo).unwrap();
    init_repo(&main_repo);
    git(
        &[
            "worktree",
            "add",
            "--quiet",
            wt_path.to_str().unwrap(),
            "-b",
            "feature-x",
        ],
        &main_repo,
    );
    std::fs::write(wt_path.join("foo.rs"), "fn main() {}").unwrap();

    let ws = Workspace::new(&main_repo, &[]).unwrap();
    let uri = format!("file://{}", ws.root().display());
    let err = ws
        .canonicalize(&uri, "scratch/branch-experiments/xyz/foo.rs")
        .unwrap_err();
    assert!(
        matches!(err, kenn_indexer::CanonicalizeError::Excluded(_)),
        "expected Excluded, got {err:?}"
    );

    // Sibling file at the same depth, NOT under the worktree, still indexes.
    let sibling = main_repo.join("scratch").join("not-a-worktree");
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(sibling.join("bar.rs"), "fn main() {}").unwrap();
    let ok = ws
        .canonicalize(&uri, "scratch/not-a-worktree/bar.rs")
        .unwrap();
    assert_eq!(ok.as_str(), "scratch/not-a-worktree/bar.rs");
}
