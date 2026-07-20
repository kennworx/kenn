//! Workspace-relative path canonicalization (section 3).
//!
//! Translates `(project_root_uri, relative_path)` pairs from a SCIP index
//! into workspace-relative paths, refusing anything outside the workspace
//! root or matching an exclude glob.
//!
//! Linked git worktrees are discovered via `git worktree list --porcelain`
//! and added to the runtime exclude set. The workspace root itself is never
//! excluded — even if it IS a linked worktree.
//!
//! # Case folding
//!
//! Paths are compared byte-exact. On macOS the default APFS filesystem is
//! case-insensitive but case-preserving; the producer relies on what the
//! indexer reports. On Linux paths are case-sensitive. On Windows callers
//! should normalize to lowercase before passing in (not done here).

use std::path::{Component, Path, PathBuf};

use std::collections::HashMap;

use globset::{Glob, GlobSet, GlobSetBuilder};
use kenn_model::Language;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CanonicalizeError {
    #[error("project_root URI must use file:// scheme: {0}")]
    BadUri(String),
    #[error("path `{path}` resolves outside the workspace root `{root}`")]
    OutsideRoot { path: String, root: String },
    #[error("path `{0}` matches a configured exclude glob")]
    Excluded(String),
    #[error("invalid glob `{pattern}`: {source}")]
    BadGlob {
        pattern: String,
        #[source]
        source: globset::Error,
    },
}

/// A workspace-relative path. Always uses `/` as separator regardless of OS
/// because that's what SCIP and downstream consumers (the wire format `./...`)
/// expect.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkspaceRelativePath(String);

impl WorkspaceRelativePath {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for WorkspaceRelativePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    /// Resolved store layout — the source of the derived-store paths
    /// where indexer intermediates (`scip-*.scip`, the kenn-dotnet JSONL
    /// stream) are written. Defaults to the in-repo layout;
    /// [`Workspace::with_layout`] sets the configured one.
    layout: kenn_store::Layout,
    /// The current indexer pass's run directory, set after
    /// `lifecycle::begin_indexing` via [`Workspace::with_run_dir`].
    /// When `Some`, [`Workspace::scip_path`] returns
    /// `<run_dir>/{slug}.scip` so the SCIP file is carried with the
    /// run on publish (§5.3). When `None`, falls back to the legacy
    /// `<derived_root>/scip-<slug>.scip` shared location — used by
    /// unit tests that don't go through a full lifecycle.
    run_dir: Option<PathBuf>,
    /// Cross-language exclude set from `[workspace].excludes` plus the
    /// hardcoded `.git/**` invariant. Consulted by `canonicalize`.
    #[expect(
        clippy::struct_field_names,
        reason = "name reflects its scope (workspace-level), distinct from per-language *_excludes"
    )]
    workspace_excludes: GlobSet,
    excluded_dirs: Vec<PathBuf>,
    tests: GlobSet,
    /// Per-language exclude sets from `[language.X].excludes`, keyed by
    /// language. A language with no configured excludes has no entry (and
    /// is therefore never excluded). Consulted via `is_excluded(language,
    /// ...)`. Markdown is absent: it owns its own discovery and never
    /// routes through here.
    language_excludes: HashMap<Language, GlobSet>,
}

/// Patterns added to every workspace's exclude set as a kenn invariant.
/// Never source code; never indexable.
const WORKSPACE_HARDCODED_EXCLUDES: &[&str] = &[".git/**", "**/.git/**"];

fn build_glob_set(label: &str, patterns: &[String]) -> Result<GlobSet, CanonicalizeError> {
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        let glob = Glob::new(pat).map_err(|source| CanonicalizeError::BadGlob {
            pattern: pat.clone(),
            source,
        })?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|source| CanonicalizeError::BadGlob {
            pattern: label.into(),
            source,
        })
}

impl Workspace {
    /// Build a workspace rooted at `root` with the supplied workspace-
    /// level exclude globs. The hardcoded `.git/**` invariant is ALWAYS
    /// merged. Per-language excludes attach via [`Self::with_language_excludes`].
    /// Linked git worktrees are auto-discovered and added to `excluded_dirs`.
    pub fn new<P: AsRef<Path>>(
        root: P,
        workspace_globs: &[String],
    ) -> Result<Self, CanonicalizeError> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|e| CanonicalizeError::BadUri(format!("canonicalize {e}")))?;
        let mut all =
            Vec::with_capacity(workspace_globs.len() + WORKSPACE_HARDCODED_EXCLUDES.len());
        all.extend(workspace_globs.iter().cloned());
        all.extend(
            WORKSPACE_HARDCODED_EXCLUDES
                .iter()
                .map(|s| (*s).to_string()),
        );
        let workspace_excludes = build_glob_set("<workspace-excludes>", &all)?;
        let excluded_dirs = discover_other_worktrees(&root);
        let layout = kenn_store::Layout::default_for(&root);
        Ok(Self {
            root,
            layout,
            run_dir: None,
            workspace_excludes,
            excluded_dirs,
            tests: GlobSet::empty(),
            language_excludes: HashMap::new(),
        })
    }

    /// Attach language-specific exclude patterns from `[language.X].excludes`.
    /// Patterns are consulted by that language's discovery walker AND
    /// its SCIP→record transform. Other languages are unaffected.
    pub fn with_language_excludes(
        mut self,
        language: Language,
        patterns: &[String],
    ) -> Result<Self, CanonicalizeError> {
        let glob_set = build_glob_set("<language-excludes>", patterns)?;
        self.language_excludes.insert(language, glob_set);
        Ok(self)
    }

    /// True if `relative_path` matches the configured exclude set for
    /// `language`. Workspace-relative path is normalized to `/`
    /// separators before matching, matching `canonicalize`. Returns
    /// `false` for languages without an attached set.
    #[must_use]
    pub fn is_excluded(&self, language: Language, relative_path: &str) -> bool {
        let normalized = relative_path.replace(std::path::MAIN_SEPARATOR, "/");
        self.language_excludes
            .get(&language)
            .is_some_and(|set| set.is_match(&normalized))
    }

    /// True if a workspace-relative path matches the cross-language
    /// workspace exclude set. Used by `canonicalize` and by the
    /// workspace-aware walker's cross-language prune step. Performs
    /// the same `/`-separator normalization as `is_excluded`.
    #[must_use]
    pub fn is_workspace_excluded(&self, relative_path: &str) -> bool {
        let normalized = relative_path.replace(std::path::MAIN_SEPARATOR, "/");
        self.workspace_excludes.is_match(&normalized)
    }

    /// Attach the resolved store [`kenn_store::Layout`] — the source of
    /// the derived-store paths indexer intermediates are written to.
    /// Defaults to the in-repo layout; `index_workspace` sets the
    /// configured one. Safe to chain after [`Self::new`].
    #[must_use]
    pub fn with_layout(mut self, layout: kenn_store::Layout) -> Self {
        self.layout = layout;
        self
    }

    /// Attach the active indexer pass's run directory — the SCIP /
    /// JSONL intermediates land inside it (§5.3 / §5.4) and are
    /// carried with the run on publish. Set by the workflow after
    /// `lifecycle::begin_indexing`.
    #[must_use]
    pub fn with_run_dir(mut self, run_dir: PathBuf) -> Self {
        self.run_dir = Some(run_dir);
        self
    }

    /// Attach test-file detection globs. Paths matching any of `test_globs`
    /// are tagged `test = true` on `FileRecord` / `SymbolRecord`. Returns
    /// the updated workspace; safe to chain after [`Self::new`].
    pub fn with_test_globs(mut self, test_globs: &[String]) -> Result<Self, CanonicalizeError> {
        let mut builder = GlobSetBuilder::new();
        for pat in test_globs {
            let glob = Glob::new(pat).map_err(|source| CanonicalizeError::BadGlob {
                pattern: pat.clone(),
                source,
            })?;
            builder.add(glob);
        }
        self.tests = builder
            .build()
            .map_err(|source| CanonicalizeError::BadGlob {
                pattern: "<tests>".into(),
                source,
            })?;
        Ok(self)
    }

    /// True if `relative_path` (workspace-relative, forward slashes) matches
    /// any configured test glob. Empty when no globs were attached.
    #[must_use]
    pub fn is_test_path(&self, relative_path: &str) -> bool {
        self.tests.is_match(relative_path)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The derived store root — where indexer intermediates land.
    #[must_use]
    pub fn derived_root(&self) -> &Path {
        self.layout.derived_root()
    }

    /// SCIP intermediate path for `slug`. When the workspace has an
    /// attached run directory (via [`Workspace::with_run_dir`]),
    /// returns `<run_dir>/{slug}.scip` so the SCIP file moves with
    /// the run on publish (§5.3). Otherwise falls back to the legacy
    /// `<derived_root>/scip-<slug>.scip` location — used by unit tests
    /// that don't go through a full lifecycle.
    #[must_use]
    pub fn scip_path(&self, slug: &str) -> PathBuf {
        match &self.run_dir {
            Some(dir) => dir.join(format!("{slug}.scip")),
            None => self.layout.scip_path(slug),
        }
    }

    /// A unique path under the active run dir (or derived root, as a
    /// fallback for unit tests that don't attach one) for a JSONL
    /// producer's stdout-redirected stream file (§5.4). `slug` names
    /// the driver (e.g. its `language_id()`); pid + counter
    /// disambiguate concurrent retries within one pass. Caller is
    /// responsible for deleting the file after ingest; failed runs
    /// take the file with them when the run dir is swept.
    pub fn jsonl_stream_path(&self, slug: &str) -> std::io::Result<PathBuf> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let parent: PathBuf = match &self.run_dir {
            Some(dir) => dir.clone(),
            None => self.layout.derived_root().to_path_buf(),
        };
        std::fs::create_dir_all(&parent)?;
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Ok(parent.join(format!("{slug}-stream-{}-{n}.jsonl", std::process::id())))
    }

    /// Absolute paths of directories the workspace treats as out-of-tree
    /// (linked git worktrees). Surfaced so unit-discovery walkers can skip
    /// them too — `canonicalize` already filters individual files, but the
    /// walker pays I/O for every recursed entry.
    #[must_use]
    pub fn excluded_dirs(&self) -> &[PathBuf] {
        &self.excluded_dirs
    }

    /// Translate a SCIP `(project_root_uri, relative_path)` pair into a
    /// workspace-relative path.
    pub fn canonicalize(
        &self,
        project_root_uri: &str,
        relative_path: &str,
    ) -> Result<WorkspaceRelativePath, CanonicalizeError> {
        let project_root = parse_file_uri(project_root_uri)?;
        let abs = project_root.join(relative_path);
        let abs = lexical_normalize(&abs);
        // `StripPrefixError` carries no detail beyond "the prefix didn't
        // match"; the OutsideRoot error already includes both paths.
        #[expect(
            clippy::map_err_ignore,
            reason = "StripPrefixError has no payload; both paths are already in OutsideRoot"
        )]
        let rel = abs
            .strip_prefix(&self.root)
            .map_err(|_| CanonicalizeError::OutsideRoot {
                path: abs.display().to_string(),
                root: self.root.display().to_string(),
            })?;

        let rel_str = rel
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");

        if self.workspace_excludes.is_match(&rel_str) {
            return Err(CanonicalizeError::Excluded(rel_str));
        }
        for d in &self.excluded_dirs {
            if abs.starts_with(d) {
                return Err(CanonicalizeError::Excluded(rel_str));
            }
        }

        Ok(WorkspaceRelativePath(rel_str))
    }
}

/// Strip a `file://` URI down to a `PathBuf`. Tolerates missing scheme by
/// treating the input as already-a-path.
fn parse_file_uri(uri: &str) -> Result<PathBuf, CanonicalizeError> {
    if let Some(rest) = uri.strip_prefix("file://") {
        return Ok(PathBuf::from(rest));
    }
    // Bare path (no scheme): allow if it looks like an absolute filesystem path.
    // Reject anything containing `://` — that's a non-file URI scheme.
    if uri.contains("://") {
        return Err(CanonicalizeError::BadUri(uri.into()));
    }
    if uri.starts_with('/') {
        return Ok(PathBuf::from(uri));
    }
    // Windows drive-letter path like `C:\foo` — keep as-is.
    if uri.len() >= 3 && uri.as_bytes().get(1) == Some(&b':') {
        return Ok(PathBuf::from(uri));
    }
    Err(CanonicalizeError::BadUri(uri.into()))
}

/// Lexical normalization: resolve `.` and `..` without touching the filesystem.
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Task 3.3a — return absolute paths of every linked git worktree of the
/// repo containing `workspace_root`, EXCLUDING `workspace_root` itself.
/// Returns empty when `workspace_root` is not in a git repo.
#[must_use]
pub fn discover_other_worktrees(workspace_root: &Path) -> Vec<PathBuf> {
    let canonical_root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    // `all_worktrees` already returns canonicalized main + linked paths;
    // drop `workspace_root` itself and any ancestor of it.
    kenn_store::git::all_worktrees(workspace_root)
        .into_iter()
        .filter(|candidate| should_exclude_other_worktree(&canonical_root, candidate))
        .collect()
}

/// True when `candidate` is a distinct sibling worktree we should add to
/// `excluded_dirs`. False when it is `canonical_root` itself OR an
/// ancestor of `canonical_root` — in the ancestor case, including it
/// would make `abs.starts_with(d)` reject every file in our workspace,
/// because we live inside that ancestor (typical layout: this worktree
/// at `<repo>/.worktrees/<name>`, with `<repo>` itself the main
/// worktree).
fn should_exclude_other_worktree(canonical_root: &Path, candidate: &Path) -> bool {
    candidate != canonical_root && !canonical_root.starts_with(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn canonicalize_subdir_relative_path() {
        // 3.4 — project root under sub-dir.
        let dir = tmp();
        fs::create_dir_all(dir.path().join("sub/Foo")).unwrap();
        fs::write(dir.path().join("sub/Foo/Bar.cs"), "").unwrap();

        let ws = Workspace::new(dir.path(), &[]).unwrap();
        let uri = format!("file://{}/sub", ws.root().display());
        let rel = ws.canonicalize(&uri, "Foo/Bar.cs").unwrap();
        assert_eq!(rel.as_str(), "sub/Foo/Bar.cs");
    }

    #[test]
    fn rejects_path_outside_workspace() {
        // 3.5
        let dir = tmp();
        let ws = Workspace::new(dir.path(), &[]).unwrap();
        let outside = format!(
            "file://{}/elsewhere",
            dir.path().parent().unwrap().display()
        );
        let err = ws.canonicalize(&outside, "Bar.cs").unwrap_err();
        assert!(matches!(err, CanonicalizeError::OutsideRoot { .. }));
    }

    #[test]
    fn rejects_excluded_workspace_glob() {
        // After `kenn-per-language-excludes`, canonicalize gates ONLY on
        // workspace-scoped excludes. Language defaults like
        // `node_modules/**` live on the per-language fields and do NOT
        // gate canonicalize. The hardcoded `.git/**` invariant DOES.
        let dir = tmp();
        fs::create_dir_all(dir.path().join(".git/foo")).unwrap();
        fs::write(dir.path().join(".git/foo/x.ts"), "").unwrap();
        let ws = Workspace::new(dir.path(), &[]).unwrap();
        let uri = format!("file://{}", ws.root().display());
        let err = ws.canonicalize(&uri, ".git/foo/x.ts").unwrap_err();
        assert!(matches!(err, CanonicalizeError::Excluded(_)));
    }

    #[test]
    fn does_not_reject_language_default_path_at_canonicalize() {
        // node_modules belongs to TypeScript's per-language excludes,
        // not workspace excludes; canonicalize lets it through. The
        // TypeScript transform calls `is_excluded(Language::TypeScript, ...)`
        // to drop it post-canonicalize.
        let dir = tmp();
        fs::create_dir_all(dir.path().join("node_modules/foo")).unwrap();
        fs::write(dir.path().join("node_modules/foo/x.ts"), "").unwrap();
        let ws = Workspace::new(dir.path(), &[]).unwrap();
        let uri = format!("file://{}", ws.root().display());
        ws.canonicalize(&uri, "node_modules/foo/x.ts").unwrap();
    }

    #[test]
    fn is_excluded_python_matches_workspace_relative() {
        let dir = tmp();
        let ws = Workspace::new(dir.path(), &[])
            .unwrap()
            .with_language_excludes(Language::Python, &["__pycache__/**".to_string()])
            .unwrap();
        assert!(ws.is_excluded(Language::Python, "__pycache__/foo.py"));
        assert!(!ws.is_excluded(Language::Python, "src/foo.py"));
    }

    #[test]
    fn is_excluded_normalizes_windows_separators() {
        // Hand-construct a relative path string with `\\` separators —
        // simulates what a Windows `Path::strip_prefix` would yield. The
        // matcher normalizes before consulting the GlobSet (patterns
        // are authored with `/`), so the match must still fire.
        let dir = tmp();
        let ws = Workspace::new(dir.path(), &[])
            .unwrap()
            .with_language_excludes(Language::Python, &["__pycache__/**".to_string()])
            .unwrap();
        let windows_style = format!("__pycache__{}foo.py", std::path::MAIN_SEPARATOR);
        // On macOS / Linux MAIN_SEPARATOR is `/` so the test trivially
        // matches; the normalization line is exercised on Windows where
        // it's `\\`. We still cover the call path here.
        assert!(ws.is_excluded(Language::Python, &windows_style));
        // Explicit backslash path also matches via normalization.
        let manual_backslash = "__pycache__\\foo.py";
        // On non-Windows MAIN_SEPARATOR is `/`, so the manual `\\`
        // string is NOT normalized — and globset does NOT match a `\\`
        // in a `/`-separated pattern. So this assertion is platform-aware:
        if std::path::MAIN_SEPARATOR == '\\' {
            assert!(ws.is_excluded(Language::Python, manual_backslash));
        }
    }

    #[test]
    fn is_excluded_does_not_consult_other_languages() {
        let dir = tmp();
        let ws = Workspace::new(dir.path(), &[])
            .unwrap()
            .with_language_excludes(Language::Python, &["__pycache__/**".to_string()])
            .unwrap();
        // Python's pattern set is NOT consulted for C#.
        assert!(!ws.is_excluded(Language::Csharp, "__pycache__/foo.cs"));
        // And TypeScript and Rust likewise see no Python excludes.
        assert!(!ws.is_excluded(Language::TypeScript, "__pycache__/foo.ts"));
        assert!(!ws.is_excluded(Language::Rust, "__pycache__/foo.rs"));
    }

    #[test]
    fn canonicalize_rejects_hardcoded_git_dir() {
        let dir = tmp();
        fs::create_dir_all(dir.path().join(".git/objects")).unwrap();
        fs::write(dir.path().join(".git/objects/abc"), "").unwrap();
        let ws = Workspace::new(dir.path(), &[]).unwrap();
        let uri = format!("file://{}", ws.root().display());
        let err = ws.canonicalize(&uri, ".git/objects/abc").unwrap_err();
        assert!(matches!(err, CanonicalizeError::Excluded(_)));
    }

    #[test]
    fn rejects_excluded_user_glob() {
        let dir = tmp();
        fs::create_dir_all(dir.path().join("vendor")).unwrap();
        fs::write(dir.path().join("vendor/x.cs"), "").unwrap();
        let ws = Workspace::new(dir.path(), &["vendor/**".into()]).unwrap();
        let uri = format!("file://{}", ws.root().display());
        let err = ws.canonicalize(&uri, "vendor/x.cs").unwrap_err();
        assert!(matches!(err, CanonicalizeError::Excluded(_)));
    }

    #[test]
    fn other_worktree_self_is_skipped() {
        let root = Path::new("/repo/.worktrees/feature");
        assert!(!should_exclude_other_worktree(root, root));
    }

    #[test]
    fn other_worktree_ancestor_is_skipped() {
        // Regression: when a worktree lives inside its main repo
        // (`<repo>/.worktrees/<name>`), `git worktree list` reports the
        // main repo as an "other" entry. Including it in `excluded_dirs`
        // makes the `abs.starts_with(d)` check exclude EVERY file in
        // this worktree, producing `documents: 0` and an empty `files`
        // Lance dataset.
        let root = Path::new("/repo/.worktrees/feature");
        let main_repo = Path::new("/repo");
        assert!(!should_exclude_other_worktree(root, main_repo));
    }

    #[test]
    fn other_worktree_sibling_is_excluded() {
        let root = Path::new("/repo/.worktrees/feature");
        let sibling = Path::new("/repo/.worktrees/other");
        assert!(should_exclude_other_worktree(root, sibling));
    }

    #[test]
    fn non_git_workspace_returns_empty_other_worktrees() {
        // 3.6c — non-git workspace skips git query, only honors explicit excludes.
        let dir = tmp();
        let others = discover_other_worktrees(dir.path());
        assert!(others.is_empty());
    }

    #[test]
    fn lexical_normalize_resolves_parent_dirs() {
        let p = lexical_normalize(Path::new("/a/b/../c/./d"));
        assert_eq!(p, PathBuf::from("/a/c/d"));
    }

    #[test]
    fn parse_file_uri_strips_scheme() {
        assert_eq!(
            parse_file_uri("file:///foo/bar").unwrap(),
            PathBuf::from("/foo/bar")
        );
        assert_eq!(
            parse_file_uri("/foo/bar").unwrap(),
            PathBuf::from("/foo/bar")
        );
        parse_file_uri("ftp://x").unwrap_err();
    }
}
