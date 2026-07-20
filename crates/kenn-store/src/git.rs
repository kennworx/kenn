//! In-process git metadata via `gix` (gix-git-backend).
//!
//! Replaces every `Command::new("git")` kenn used for staleness, worktree
//! resolution, and root discovery. Reading the repository directly removes the
//! dependency on the `git` binary, `PATH`, `safe.directory`, `core.quotepath`,
//! and porcelain parsing. Each function discovers the repo by walking upward
//! from the given path (like `git` does), returning the "not a git repo" signal
//! (`None` / empty `Vec`) on any failure so the existing non-git fallbacks
//! stand unchanged.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Open the repository containing `path` by upward discovery (like `git`), or
/// `None` when `path` is not inside a git working tree.
fn discover(path: &Path) -> Option<gix::Repository> {
    gix::discover(path).ok()
}

/// Canonicalize `p`, falling back to `p` itself when it cannot be resolved —
/// matches `git rev-parse`'s absolute, symlink-resolved output.
fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// HEAD commit id as a full hex string — replaces `git rev-parse HEAD`. `None`
/// when `path` is not a repo or HEAD is unborn (no commit yet).
#[must_use]
pub fn head_id(path: &Path) -> Option<String> {
    Some(discover(path)?.head_id().ok()?.to_string())
}

/// The working-tree root — replaces `git rev-parse --show-toplevel`. `None` for
/// a bare repo or a non-repo.
#[must_use]
pub fn work_dir(path: &Path) -> Option<PathBuf> {
    Some(canonical(discover(path)?.workdir()?))
}

/// The repository's git directory — resolved via upward discovery so it follows
/// `GIT_DIR` and a linked worktree's gitdir file rather than assuming
/// `<root>/.git`. Used to exclude git metadata from indexing without hardcoding
/// the literal `.git` path. `None` outside a repo.
#[must_use]
pub fn git_dir(path: &Path) -> Option<PathBuf> {
    Some(canonical(discover(path)?.git_dir()))
}

/// The main worktree's path — the first entry `git worktree list --porcelain`
/// prints. Derived from the shared common git dir (`<main>/.git`), whose parent
/// is the main worktree, so it needs no worktree enumeration. `None` when
/// `path` is not in a repo (or the common dir has no parent).
#[must_use]
pub fn main_worktree(path: &Path) -> Option<PathBuf> {
    let repo = discover(path)?;
    // `common_dir()` can be unnormalized for a linked worktree (git's
    // `commondir` is a relative `../..`), so canonicalize BEFORE taking the
    // parent — otherwise stripping a component leaves an unresolved `..`.
    let common = canonical(repo.common_dir());
    Some(common.parent()?.to_path_buf())
}

/// Every worktree of the repo — the main worktree plus each linked worktree
/// (the set `git worktree list` prints). Empty when `path` is not in a repo.
#[must_use]
pub fn all_worktrees(path: &Path) -> Vec<PathBuf> {
    let Some(repo) = discover(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // Canonicalize the common dir before taking its parent (see `main_worktree`).
    let common = canonical(repo.common_dir());
    if let Some(main) = common.parent() {
        out.push(main.to_path_buf());
    }
    if let Ok(linked) = repo.worktrees() {
        for wt in linked {
            if let Ok(base) = wt.base() {
                out.push(canonical(&base));
            }
        }
    }
    out
}

/// Repo-relative paths of tracked files reported modified / deleted / renamed —
/// the tracked, non-`??` set `git status --porcelain` prints, spanning both
/// staged (HEAD-vs-index) and unstaged (index-vs-worktree) changes. Untracked
/// files are excluded by disabling the dirwalk, so a large untracked tree
/// (`node_modules/`) costs nothing (design D2). Sorted + deduplicated. `None`
/// when `path` is not a repo.
#[must_use]
pub fn tracked_modified(path: &Path) -> Option<Vec<String>> {
    let repo = discover(path)?;
    // Default `status()` compares HEAD→index→worktree (so a *staged* change to
    // a clean worktree is still reported); `UntrackedFiles::None` turns off the
    // dirwalk entirely, so only tracked entries are produced.
    let platform = repo
        .status(gix::progress::Discard)
        .ok()?
        .untracked_files(gix::status::UntrackedFiles::None);
    let iter = platform.into_iter(None::<gix::bstr::BString>).ok()?;
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for item in iter {
        let Ok(item) = item else { continue };
        let rela = match item {
            // Index-vs-worktree change (unstaged). With the dirwalk off this is
            // only tracked modifications/deletions/renames, never untracked.
            gix::status::Item::IndexWorktree(iw) => iw.rela_path().to_string(),
            // HEAD-tree-vs-index change (staged).
            gix::status::Item::TreeIndex(change) => change.location().to_string(),
        };
        paths.insert(rela);
    }
    Some(paths.into_iter().collect())
}
