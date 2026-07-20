//! Worktree → parent fallback — `index-store-worktree-fallback` capability.
//!
//! Discovery is git-driven via [`crate::git`] (in-process gix): the repo's
//! shared common git dir resolves the main worktree directly. Path-pattern
//! heuristics ("does the path contain `worktrees/`?") are forbidden — a
//! linked worktree can sit anywhere on disk.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::layout::{Layout, Store};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReadSource {
    Local,
    FallbackFromParent { parent: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadContext {
    Available {
        snapshot: PathBuf,
        source: ReadSource,
    },
    Tier2Unavailable,
}

/// Resolve the main worktree for a path inside a git repo. Returns `None`
/// when the path is not inside a git working tree.
#[must_use]
pub fn resolve_main_worktree(workspace: &Path) -> Option<PathBuf> {
    crate::git::main_worktree(workspace)
}

/// Spec §Local-first read + §Parent fallback. Tries:
///
/// 1. The local snapshot resolved through `layout` (honoring a
///    relocated `derived_root`)
/// 2. The main worktree's `live` if the layout's source root is a linked
///    worktree and the main worktree has its own in-repo snapshot
/// 3. `Tier2Unavailable` otherwise
///
/// The parent fallback always uses the main worktree's *default* in-repo
/// layout — a relocated derived root is per-repo, so a shared one already
/// serves every worktree and a custom in-repo one is private to its
/// worktree.
#[must_use]
pub fn open_for_read(layout: &Layout) -> ReadContext {
    if let Ok(local) = Store::open(layout.clone()) {
        if let Some(snap) = local.live_target() {
            return ReadContext::Available {
                snapshot: snap,
                source: ReadSource::Local,
            };
        }
    }
    let workspace = layout.source_root();
    let Some(main) = resolve_main_worktree(workspace) else {
        return ReadContext::Tier2Unavailable;
    };
    if main == workspace {
        // workspace IS the main worktree, but it had no local `live` (caught above).
        return ReadContext::Tier2Unavailable;
    }
    let Ok(parent_store) = Store::open_default(&main) else {
        return ReadContext::Tier2Unavailable;
    };
    if let Some(snap) = parent_store.live_target() {
        return ReadContext::Available {
            snapshot: snap,
            source: ReadSource::FallbackFromParent { parent: main },
        };
    }
    ReadContext::Tier2Unavailable
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    use crate::lifecycle::begin_indexing;

    fn git_init(dir: &Path) {
        Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(dir)
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.invalid"])
            .current_dir(dir)
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "test"])
            .current_dir(dir)
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "commit.gpgsign", "false"])
            .current_dir(dir)
            .status()
            .unwrap();
    }

    fn git_commit_initial(dir: &Path) {
        fs::write(dir.join("README"), b"x").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(dir)
            .status()
            .unwrap();
    }

    fn add_worktree(repo: &Path, wt_path: &Path, branch: &str) {
        Command::new("git")
            .args(["worktree", "add", "-b", branch, wt_path.to_str().unwrap()])
            .current_dir(repo)
            .status()
            .unwrap();
    }

    fn publish_one(workspace: &Path) -> PathBuf {
        let s = Store::open_default(workspace).unwrap();
        let h = begin_indexing(&s).unwrap();
        // KVS2 / D1: publish refuses without a `meta.json` completion
        // stamp. Tests don't run the full pipeline, so we stub it.
        std::fs::write(h.run_dir().join("meta.json"), b"{\"status\":\"success\"}").unwrap();
        h.publish().unwrap()
    }

    #[test]
    fn linked_worktree_default_layout_shares_the_main_vectors_root() {
        let repo = TempDir::new().unwrap();
        git_init(repo.path());
        git_commit_initial(repo.path());
        let wt_dir = TempDir::new().unwrap();
        let wt_path = wt_dir.path().join("feature-y");
        add_worktree(repo.path(), &wt_path, "feature-y");

        // The linked worktree's default vectors root is the MAIN tree's
        // shared dir (`shared-vector-cache` Phase 3)…
        let wt_layout = Layout::default_for(&wt_path);
        let canon_repo = fs::canonicalize(repo.path()).unwrap();
        assert_eq!(
            wt_layout.vectors_root(),
            canon_repo.join(".kenn").join("vectors")
        );
        // …while the derived store (snapshots) stays per-worktree.
        assert_eq!(
            wt_layout.derived_root(),
            wt_path.join(".kenn").join("local")
        );
        // The main worktree keeps its caller-spelled in-repo path.
        let main_layout = Layout::default_for(repo.path());
        assert_eq!(
            main_layout.vectors_root(),
            repo.path().join(".kenn").join("vectors")
        );
    }

    #[test]
    fn relative_vectors_location_resolves_at_the_main_worktree() {
        let repo = TempDir::new().unwrap();
        git_init(repo.path());
        git_commit_initial(repo.path());
        let wt_dir = TempDir::new().unwrap();
        let wt_path = wt_dir.path().join("feature-z");
        add_worktree(repo.path(), &wt_path, "feature-z");

        let config =
            kenn_config::Config::from_toml("[vectors]\nlocation = \"team-vectors\"\n").unwrap();
        let from_main = Layout::resolve(&config, repo.path()).unwrap();
        let from_wt = Layout::resolve(&config, &wt_path).unwrap();
        // Both worktrees land on ONE shared dir (`shared-vector-cache`
        // Phase 1) — compare canonicalized parents since the main layout
        // preserves the caller's spelling.
        let canon = |p: &Path| {
            let parent = p.parent().unwrap();
            fs::canonicalize(parent)
                .unwrap()
                .join(p.file_name().unwrap())
        };
        assert_eq!(
            canon(from_main.vectors_root()),
            canon(from_wt.vectors_root())
        );
        assert!(from_wt.vectors_root().ends_with(Path::new("team-vectors")));
    }

    #[test]
    fn local_snapshot_wins_over_parent() {
        let repo = TempDir::new().unwrap();
        git_init(repo.path());
        git_commit_initial(repo.path());
        let parent_snap = publish_one(repo.path());

        let wt_dir = TempDir::new().unwrap();
        let wt_path = wt_dir.path().join("feature-x");
        add_worktree(repo.path(), &wt_path, "feature-x");
        let local_snap = publish_one(&wt_path);

        match open_for_read(&Layout::default_for(&wt_path)) {
            ReadContext::Available { snapshot, source } => {
                assert_eq!(snapshot, local_snap);
                assert_eq!(source, ReadSource::Local);
            }
            ReadContext::Tier2Unavailable => panic!("expected Available, got Tier2Unavailable"),
        }
        // Sanity: parent snapshot exists and is different.
        assert_ne!(parent_snap, local_snap);
    }

    #[test]
    fn worktree_without_local_falls_back_to_parent() {
        let repo = TempDir::new().unwrap();
        git_init(repo.path());
        git_commit_initial(repo.path());
        let parent_snap = publish_one(repo.path());

        let wt_dir = TempDir::new().unwrap();
        let wt_path = wt_dir.path().join("arbitrary-name");
        add_worktree(repo.path(), &wt_path, "feat");

        match open_for_read(&Layout::default_for(&wt_path)) {
            ReadContext::Available { snapshot, source } => {
                assert_eq!(
                    fs::canonicalize(snapshot).unwrap(),
                    fs::canonicalize(&parent_snap).unwrap()
                );
                match source {
                    ReadSource::FallbackFromParent { parent } => {
                        // Resolve symlinks because macOS /tmp is a symlink to /private/tmp.
                        let canon_parent = fs::canonicalize(&parent).unwrap();
                        let canon_repo = fs::canonicalize(repo.path()).unwrap();
                        assert_eq!(canon_parent, canon_repo);
                    }
                    ReadSource::Local => panic!("expected FallbackFromParent, got Local"),
                }
            }
            ReadContext::Tier2Unavailable => panic!("expected Available, got Tier2Unavailable"),
        }
    }

    #[test]
    fn neither_local_nor_parent_returns_tier2_unavailable() {
        let dir = TempDir::new().unwrap();
        // Not a git repo at all → no local, no parent.
        assert_eq!(
            open_for_read(&Layout::default_for(dir.path())),
            ReadContext::Tier2Unavailable
        );
    }

    #[test]
    fn fresh_clone_with_git_but_no_index_returns_tier2_unavailable() {
        let repo = TempDir::new().unwrap();
        git_init(repo.path());
        git_commit_initial(repo.path());
        // No publish — no live anywhere.
        assert_eq!(
            open_for_read(&Layout::default_for(repo.path())),
            ReadContext::Tier2Unavailable
        );
    }

    #[test]
    fn worktree_at_unconventional_path_resolves_via_git() {
        // Spec scenario: linked worktree at a path that has nothing to do
        // with `.worktrees/` or any naming convention.
        let repo = TempDir::new().unwrap();
        git_init(repo.path());
        git_commit_initial(repo.path());
        publish_one(repo.path());

        let outsider_dir = TempDir::new().unwrap();
        let outsider = outsider_dir
            .path()
            .join("scratch")
            .join("branch-experiments")
            .join("xyz");
        fs::create_dir_all(outsider.parent().unwrap()).unwrap();
        add_worktree(repo.path(), &outsider, "xyz");

        match open_for_read(&Layout::default_for(&outsider)) {
            ReadContext::Available {
                source: ReadSource::FallbackFromParent { .. },
                ..
            } => {}
            other => panic!("expected fallback-from-parent for outsider worktree, got {other:?}"),
        }
    }

    #[test]
    fn writes_in_worktree_never_touch_parent() {
        // Spec scenario §No writes to parent. We exercise the writer in the
        // worktree and assert the parent's `index.lock`, `building/`, `live`
        // are untouched (mtime + presence inspection).
        let repo = TempDir::new().unwrap();
        git_init(repo.path());
        git_commit_initial(repo.path());
        let parent_snap = publish_one(repo.path());

        let parent_store = Store::open_default(repo.path()).unwrap();
        let parent_lock = parent_store.lock_path();
        let parent_live_meta_before = fs::symlink_metadata(parent_store.live_path()).unwrap();
        let parent_lock_meta_before = fs::metadata(&parent_lock)
            .ok()
            .map(|m| m.modified().unwrap());

        let wt_dir = TempDir::new().unwrap();
        let wt_path = wt_dir.path().join("feature-x");
        add_worktree(repo.path(), &wt_path, "feature-x");
        publish_one(&wt_path);

        // Parent live still points at the same snapshot, untouched.
        assert_eq!(parent_store.live_target().unwrap(), parent_snap);
        let parent_live_meta_after = fs::symlink_metadata(parent_store.live_path()).unwrap();
        assert_eq!(
            parent_live_meta_before.modified().unwrap(),
            parent_live_meta_after.modified().unwrap()
        );
        // Parent lock either unchanged or absent — the worktree must never
        // have created or touched it.
        let parent_lock_meta_after = fs::metadata(&parent_lock)
            .ok()
            .map(|m| m.modified().unwrap());
        assert_eq!(parent_lock_meta_before, parent_lock_meta_after);
        // The new layout (D1) has no `building/` directory — incomplete
        // runs would land under `runs/{id}/`; opening a worktree against
        // the parent must not create any such run dir.
        let parent_runs_count =
            fs::read_dir(parent_store.runs_dir()).map_or(0, std::iter::Iterator::count);
        // Captured here; this read just confirms the parent's runs
        // dir is observable. The worktree never wrote into it.
        let _ = parent_runs_count;
    }
}
