//! Reindex skip — `workspace-staleness` capability.
//!
//! A workspace's [`StalenessKey`] takes one of two concrete forms. For a
//! git repository it is `(HEAD commit, sorted [(path, xxhash)] over
//! tracked-modified files)` — equality means "no commit advance and no
//! edits since the last index". For a non-git workspace it is a
//! `stat`-based fingerprint of the source tree. [`StalenessKey::Unknown`]
//! covers the case where neither can be determined.
//!
//! Both forms are cheap: the git form hashes only the *tracked* files
//! reported modified (untracked files are excluded, so a large untracked
//! tree like `node_modules/` costs nothing); the tree form is one `stat`
//! per file and never reads file contents.
//!
//! All git metadata is read in-process via [`crate::git`] (gix), not by
//! spawning the `git` binary — so the git form works regardless of `PATH`,
//! `safe.directory`, `core.quotepath`, or git version.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::Xxh3;
use xxhash_rust::xxh64::xxh64;

/// A workspace freshness key. Two keys that [`matches`](StalenessKey::matches)
/// mean the workspace has not changed in a way that warrants a reindex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StalenessKey {
    /// Git workspace — the `HEAD` commit plus sorted hashes of every
    /// *tracked* file `git status` reports modified. Untracked files are
    /// excluded (D7). `config_sig` is the indexing-affecting config hash
    /// (`Config::indexing_signature`), so a language-config change forces
    /// a reindex even with unchanged git state.
    Git {
        head: String,
        dirty_files: Vec<DirtyFile>,
        config_sig: u64,
    },
    /// Non-git workspace — a `stat`-based fingerprint of the source tree,
    /// plus the indexing-affecting config hash (see `Git::config_sig`).
    Tree { fingerprint: u64, config_sig: u64 },
    /// Neither form is resolvable (not a git repo, and the tree walk
    /// failed). Never matches anything — forces a conservative reindex.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyFile {
    pub path: String,
    pub xxhash: u64,
}

impl StalenessKey {
    /// True only when both keys carry the same form and equal contents:
    /// two `Git` keys match iff their `head`, `dirty_files`, AND
    /// `config_sig` are equal; two `Tree` keys match iff their
    /// `fingerprint` AND `config_sig` are equal. Every mixed pairing, and
    /// any pairing involving `Unknown`, is false — a non-match costs only
    /// one redundant, always-safe reindex.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Git {
                    head: a,
                    dirty_files: af,
                    config_sig: ac,
                },
                Self::Git {
                    head: b,
                    dirty_files: bf,
                    config_sig: bc,
                },
            ) => a == b && af == bf && ac == bc,
            (
                Self::Tree {
                    fingerprint: a,
                    config_sig: ac,
                },
                Self::Tree {
                    fingerprint: b,
                    config_sig: bc,
                },
            ) => a == b && ac == bc,
            _ => false,
        }
    }
}

/// Sentinel xxhash for a dirty tracked file that is absent or unreadable — a
/// deletion. A deleted file appears in `git status` but has no content to hash;
/// mapping it to a fixed sentinel (rather than dropping it) keeps the deletion
/// in the key. Dropping it would leave the key equal to the pre-delete state —
/// where the file was clean and absent from the dirty set — so the reindex
/// would be wrongly skipped. A collision with a real content hash is a
/// 1-in-2^64 non-event and harmless (the entry's `path` still differs).
const DELETED_OR_UNREADABLE: u64 = u64::MAX;

/// One `dirty_files` entry: the file's content hash when readable, else the
/// [`DELETED_OR_UNREADABLE`] sentinel.
fn dirty_entry(workspace: &Path, rel: &str) -> DirtyFile {
    let abs = workspace.join(rel);
    let xxhash = std::fs::read(&abs).map_or(DELETED_OR_UNREADABLE, |b| xxh64(&b, 0));
    DirtyFile {
        path: rel.to_owned(),
        xxhash,
    }
}

/// Compute the workspace's current [`StalenessKey`]: the `Git` form when the
/// workspace is a git repository ([`crate::git::head_id`] resolves), the `Tree`
/// form otherwise, and `Unknown` only when the tree walk itself fails.
/// `config_sig` is the indexing-affecting config hash
/// ([`kenn_config::Config::indexing_signature`]) folded into the key so a
/// language-config change forces a reindex.
#[must_use]
pub fn compute_staleness_key(workspace: &Path, config_sig: u64) -> StalenessKey {
    if let Some(head) = crate::git::head_id(workspace) {
        let dirty = crate::git::tracked_modified(workspace).unwrap_or_default();
        let mut dirty_files: Vec<DirtyFile> = dirty
            .iter()
            .map(|rel| dirty_entry(workspace, rel))
            .collect();
        dirty_files.sort_by(|a, b| a.path.cmp(&b.path));
        return StalenessKey::Git {
            head,
            dirty_files,
            config_sig,
        };
    }
    match tree_fingerprint(workspace) {
        Some(fingerprint) => StalenessKey::Tree {
            fingerprint,
            config_sig,
        },
        None => StalenessKey::Unknown,
    }
}

/// xxh64 hex digest of a file's content, or `None` when the path is unreadable
/// or a directory. Shares the working-tree staleness hash ([`xxh64`] with seed
/// `0`) so anchor content-drift and index staleness agree on file identity.
#[must_use]
pub fn file_content_sha(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(format!("{:016x}", xxh64(&bytes, 0)))
}

/// Directory leaf names always skipped when walking the workspace tree.
///
/// Used by the staleness tree-fingerprint walk and by the MCP
/// file-watcher's path filter. A fixed list, deliberately *not* the
/// configurable `[exclude] globs` — consulting those would thread a
/// `GlobSet` through every caller, and the watcher needs sensible
/// defaults even when the user has no `kenn.toml`.
///
/// `.kenn` MUST be present: indexing writes snapshots under `.kenn/`,
/// and a walk that counted them would change the fingerprint on every
/// publish (and the watcher would trigger a reindex storm).
pub const WORKSPACE_SKIP_DIRS: &[&str] = &["node_modules", "target", "bin", "obj", ".git", ".kenn"];

/// A `stat`-only fingerprint of the source tree, for a non-git workspace.
/// Returns `None` only when the workspace root itself cannot be read.
fn tree_fingerprint(workspace: &Path) -> Option<u64> {
    // Fail (→ `Unknown`) only if the root is unreadable; an unreadable
    // sub-directory is skipped, not fatal.
    std::fs::read_dir(workspace).ok()?;
    let mut hasher = Xxh3::new();
    fingerprint_dir(workspace, workspace, &mut hasher);
    Some(hasher.digest())
}

/// Walk `dir` depth-first in deterministic (sorted) order, folding every
/// regular file's `(workspace-relative path, mtime_nanos, size)` into
/// `hasher`. Never reads file contents. Uses `symlink_metadata`, so a
/// symlink — to a file or a directory — is never traversed and a symlink
/// cycle cannot hang the walk.
fn fingerprint_dir(root: &Path, dir: &Path, hasher: &mut Xxh3) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = read_dir.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            let leaf = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if !WORKSPACE_SKIP_DIRS.contains(&leaf) {
                fingerprint_dir(root, &path, hasher);
            }
        } else if meta.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            hasher.update(rel.to_string_lossy().as_bytes());
            hasher.update(&[0]); // path/metadata delimiter
            let mtime_nanos = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0u128, |d| d.as_nanos());
            hasher.update(&mtime_nanos.to_le_bytes());
            hasher.update(&meta.len().to_le_bytes());
        }
        // Symlinks and other entry kinds are neither dirs nor regular
        // files here — skipped, so the walk never follows a link.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    fn git_init(dir: &Path) {
        Command::new("git")
            .args(["init", "-q"])
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

    fn git_commit_all(dir: &Path, message: &str) {
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(dir)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-q", "-m", message])
            .current_dir(dir)
            .status()
            .unwrap();
    }

    /// Destructure a `Git` key into `(head, dirty_files)`, panicking on any
    /// other form — keeps the git-path regression tests terse.
    fn git_parts(key: &StalenessKey) -> (&str, &[DirtyFile]) {
        match key {
            StalenessKey::Git {
                head, dirty_files, ..
            } => (head, dirty_files),
            other => panic!("expected a Git key, got {other:?}"),
        }
    }

    #[test]
    fn dirty_entry_hashes_present_and_sentinels_missing() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.rs"), b"hello").unwrap();
        assert_eq!(dirty_entry(dir.path(), "a.rs").xxhash, xxh64(b"hello", 0));
        // A missing (deleted) file is the sentinel, not dropped.
        assert_eq!(
            dirty_entry(dir.path(), "missing.rs").xxhash,
            DELETED_OR_UNREADABLE
        );
    }

    /// Regression: deleting a tracked file MUST change the git staleness key.
    /// A deletion shows in `git status` but has no content to hash; dropping it
    /// would leave the key equal to the clean pre-delete state, wrongly skipping
    /// the reindex.
    #[test]
    fn deleting_a_tracked_file_changes_the_key() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("keep.rs"), b"keep").unwrap();
        fs::write(dir.path().join("gone.rs"), b"gone").unwrap();
        git_commit_all(dir.path(), "init");

        let clean = compute_staleness_key(dir.path(), 0);
        fs::remove_file(dir.path().join("gone.rs")).unwrap();
        let deleted = compute_staleness_key(dir.path(), 0);

        assert!(!clean.matches(&deleted), "a deletion must change the key");
        let (_, dirty) = git_parts(&deleted);
        let entry = dirty
            .iter()
            .find(|f| f.path == "gone.rs")
            .expect("deleted file is represented in the key");
        assert_eq!(entry.xxhash, DELETED_OR_UNREADABLE);
    }

    // --- matches() (task 1.3) ---------------------------------------

    #[test]
    fn equal_tree_keys_match() {
        let a = StalenessKey::Tree {
            fingerprint: 42,
            config_sig: 0,
        };
        let b = StalenessKey::Tree {
            fingerprint: 42,
            config_sig: 0,
        };
        assert!(a.matches(&b));
    }

    #[test]
    fn differing_tree_keys_do_not_match() {
        let a = StalenessKey::Tree {
            fingerprint: 1,
            config_sig: 0,
        };
        let b = StalenessKey::Tree {
            fingerprint: 2,
            config_sig: 0,
        };
        assert!(!a.matches(&b));
    }

    /// The core fix: two keys with identical git/tree state but a
    /// different `config_sig` MUST NOT match (a language-config change
    /// forces a reindex), while an identical `config_sig` matches.
    #[test]
    fn config_sig_gates_the_match() {
        let same_a = StalenessKey::Tree {
            fingerprint: 7,
            config_sig: 100,
        };
        let same_b = StalenessKey::Tree {
            fingerprint: 7,
            config_sig: 100,
        };
        assert!(same_a.matches(&same_b), "same config_sig → match");

        let diff = StalenessKey::Tree {
            fingerprint: 7,
            config_sig: 200,
        };
        assert!(
            !same_a.matches(&diff),
            "different config_sig (same tree state) → no match"
        );

        // Same gate on the git form.
        let git_same = StalenessKey::Git {
            head: "abc".into(),
            dirty_files: vec![],
            config_sig: 1,
        };
        let git_diff = StalenessKey::Git {
            head: "abc".into(),
            dirty_files: vec![],
            config_sig: 2,
        };
        assert!(!git_same.matches(&git_diff), "git config_sig gates too");
    }

    #[test]
    fn a_git_key_and_a_tree_key_never_match() {
        let git = StalenessKey::Git {
            head: "abc".into(),
            dirty_files: vec![],
            config_sig: 0,
        };
        let tree = StalenessKey::Tree {
            fingerprint: 0,
            config_sig: 0,
        };
        assert!(!git.matches(&tree));
        assert!(!tree.matches(&git));
    }

    #[test]
    fn unknown_never_matches() {
        let unknown = StalenessKey::Unknown;
        assert!(!unknown.matches(&StalenessKey::Unknown));
        assert!(!unknown.matches(&StalenessKey::Tree {
            fingerprint: 0,
            config_sig: 0,
        }));
        assert!(!unknown.matches(&StalenessKey::Git {
            head: "abc".into(),
            dirty_files: vec![],
            config_sig: 0,
        }));
    }

    // --- tree fingerprint (task 2.3) --------------------------------

    #[test]
    fn editing_a_file_changes_the_fingerprint() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        let before = compute_staleness_key(dir.path(), 0);

        // A different-length rewrite changes `size`, so the fingerprint
        // differs regardless of mtime resolution.
        fs::write(dir.path().join("a.txt"), b"hello world").unwrap();
        let after = compute_staleness_key(dir.path(), 0);

        assert!(matches!(before, StalenessKey::Tree { .. }));
        assert!(!before.matches(&after));
    }

    #[test]
    fn an_unchanged_tree_yields_a_stable_fingerprint() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), b"x").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub").join("b.txt"), b"y").unwrap();

        let k1 = compute_staleness_key(dir.path(), 0);
        let k2 = compute_staleness_key(dir.path(), 0);
        assert!(matches!(k1, StalenessKey::Tree { .. }));
        assert!(k1.matches(&k2));
    }

    /// Same workspace, different `config_sig` → keys do NOT match: a
    /// language-config change forces a reindex even on an unchanged tree.
    #[test]
    fn changing_config_sig_breaks_the_match() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), b"x").unwrap();

        let with_a = compute_staleness_key(dir.path(), 111);
        let with_a_again = compute_staleness_key(dir.path(), 111);
        let with_b = compute_staleness_key(dir.path(), 222);

        assert!(with_a.matches(&with_a_again), "same config_sig still skips");
        assert!(
            !with_a.matches(&with_b),
            "a changed config_sig forces a reindex on the same tree"
        );
    }

    #[test]
    fn writing_under_dot_kenn_does_not_perturb_the_fingerprint() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let before = compute_staleness_key(dir.path(), 0);

        // Simulate an index run writing a snapshot under `.kenn/`.
        let snap = dir.path().join(".kenn").join("local").join("snapshots");
        fs::create_dir_all(&snap).unwrap();
        fs::write(snap.join("data"), b"index output").unwrap();
        let after = compute_staleness_key(dir.path(), 0);

        assert!(before.matches(&after), ".kenn/ must be skipped by the walk");
    }

    // --- git path regression (task 4.2) -----------------------------

    #[test]
    fn non_git_workspace_yields_a_tree_key() {
        let dir = TempDir::new().unwrap();
        let key = compute_staleness_key(dir.path(), 0);
        assert!(matches!(key, StalenessKey::Tree { .. }));
    }

    #[test]
    fn clean_repo_keys_match_across_invocations() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("a.rs"), b"fn main() {}").unwrap();
        git_commit_all(dir.path(), "init");

        let k1 = compute_staleness_key(dir.path(), 0);
        let k2 = compute_staleness_key(dir.path(), 0);
        let (head, dirty) = git_parts(&k1);
        assert!(!head.is_empty());
        assert_eq!(dirty, &[]);
        assert!(k1.matches(&k2));
    }

    #[test]
    fn editing_a_file_changes_the_git_key() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("a.rs"), b"fn main() {}").unwrap();
        git_commit_all(dir.path(), "init");
        let before = compute_staleness_key(dir.path(), 0);

        fs::write(dir.path().join("a.rs"), b"fn main() { println!(\"hi\"); }").unwrap();
        let after = compute_staleness_key(dir.path(), 0);

        let (before_head, _) = git_parts(&before);
        let (after_head, after_dirty) = git_parts(&after);
        assert_eq!(before_head, after_head);
        assert_eq!(after_dirty.len(), 1);
        assert_eq!(after_dirty[0].path, "a.rs");
        assert!(!before.matches(&after));
    }

    #[test]
    fn untracked_file_does_not_change_the_key() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("a.rs"), b"x").unwrap();
        git_commit_all(dir.path(), "init");
        let before = compute_staleness_key(dir.path(), 0);

        // A new untracked file MUST NOT perturb the git key (D7): the key
        // hashes tracked-modified files only, so untracked scratch
        // (node_modules, build output, tmp clones) can never inflate it.
        fs::write(dir.path().join("b.rs"), b"y").unwrap();
        let after = compute_staleness_key(dir.path(), 0);

        assert!(
            before.matches(&after),
            "an untracked file must not change the key"
        );
        let (_, dirty) = git_parts(&after);
        assert!(
            dirty.iter().all(|d| d.path != "b.rs"),
            "untracked file must not appear in the dirty set"
        );
    }

    #[test]
    fn dirty_files_sorted_for_deterministic_equality() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("a.rs"), b"x").unwrap();
        git_commit_all(dir.path(), "init");
        fs::write(dir.path().join("z.rs"), b"z").unwrap();
        fs::write(dir.path().join("a.rs"), b"x2").unwrap();
        let key = compute_staleness_key(dir.path(), 0);
        let (_, dirty) = git_parts(&key);
        let names: Vec<_> = dirty.iter().map(|d| d.path.clone()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    // --- gix parity gate (gix-git-backend D2) -----------------------

    /// Reproduce the OLD subprocess key's tracked path set from
    /// `git status --porcelain -z` (tracked = non-`??` entries, 3-byte prefix
    /// stripped) — the reference the in-process gix port must match.
    fn git_porcelain_tracked_set(dir: &Path) -> std::collections::BTreeSet<String> {
        let out = Command::new("git")
            .args(["status", "--porcelain", "-z"])
            .current_dir(dir)
            .output()
            .unwrap();
        let mut set = std::collections::BTreeSet::new();
        for entry in out.stdout.split(|b| *b == 0) {
            if entry.starts_with(b"??") {
                continue;
            }
            let Some(rest) = entry.get(3..) else { continue };
            let path = String::from_utf8_lossy(rest).to_string();
            if !path.is_empty() {
                set.insert(path);
            }
        }
        set
    }

    /// D2 acceptance gate: `git::tracked_modified` reproduces exactly the
    /// tracked-modified set `git status --porcelain` reports (modifications +
    /// deletions, nested paths), and a large untracked directory never enters
    /// it.
    #[test]
    fn gix_tracked_set_matches_git_porcelain_and_ignores_untracked() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("keep.rs"), b"keep").unwrap();
        fs::write(dir.path().join("edit.rs"), b"v1").unwrap();
        fs::write(dir.path().join("gone.rs"), b"gone").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/nested.rs"), b"n1").unwrap();
        git_commit_all(dir.path(), "init");

        // Working-tree changes: modify, delete, modify-nested.
        fs::write(dir.path().join("edit.rs"), b"v2-changed").unwrap();
        fs::remove_file(dir.path().join("gone.rs")).unwrap();
        fs::write(dir.path().join("sub/nested.rs"), b"n2-changed").unwrap();
        // A large untracked dir + an untracked file that MUST NOT appear.
        fs::create_dir(dir.path().join("node_modules")).unwrap();
        for i in 0..50 {
            fs::write(
                dir.path().join("node_modules").join(format!("f{i}.js")),
                b"x",
            )
            .unwrap();
        }
        fs::write(dir.path().join("untracked.rs"), b"new").unwrap();

        let gix_set: std::collections::BTreeSet<String> = crate::git::tracked_modified(dir.path())
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(
            gix_set,
            git_porcelain_tracked_set(dir.path()),
            "gix tracked set must match git porcelain byte-for-byte"
        );
        let expected: std::collections::BTreeSet<String> = ["edit.rs", "gone.rs", "sub/nested.rs"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(gix_set, expected);
        assert!(
            !gix_set.iter().any(|p| p.starts_with("node_modules")),
            "untracked dir must never appear"
        );
    }

    /// A *staged* modification whose worktree matches the index (so index↔worktree
    /// is clean) must still change the key — proving the HEAD-tree↔index (staged)
    /// comparison is included, not just index↔worktree.
    #[test]
    fn a_staged_modification_changes_the_key() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("a.rs"), b"v1").unwrap();
        git_commit_all(dir.path(), "init");
        let clean = compute_staleness_key(dir.path(), 0);

        fs::write(dir.path().join("a.rs"), b"v2-staged").unwrap();
        Command::new("git")
            .args(["add", "a.rs"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        let staged = compute_staleness_key(dir.path(), 0);

        assert!(
            !clean.matches(&staged),
            "a staged modification must change the key"
        );
        let (_, dirty) = git_parts(&staged);
        assert!(
            dirty.iter().any(|d| d.path == "a.rs"),
            "the staged file is in the dirty set"
        );
    }

    /// A staged rename must change the key (staleness never wrongly skips a
    /// reindex after a rename).
    #[test]
    fn a_staged_rename_changes_the_key() {
        let dir = TempDir::new().unwrap();
        git_init(dir.path());
        fs::write(dir.path().join("old.rs"), b"content").unwrap();
        git_commit_all(dir.path(), "init");
        let clean = compute_staleness_key(dir.path(), 0);

        Command::new("git")
            .args(["mv", "old.rs", "new.rs"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        let renamed = compute_staleness_key(dir.path(), 0);

        assert!(
            !clean.matches(&renamed),
            "a staged rename must change the key"
        );
    }
}
