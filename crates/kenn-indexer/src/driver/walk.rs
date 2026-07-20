//! Directory walkers shared by the language drivers. Both prune excluded
//! directories before `read_dir`, so populated `.venv/` or `target/` cost
//! zero IO.

use std::path::PathBuf;

use crate::canonicalize::Workspace;

/// Generic depth-first walker. Skips any directory whose leaf name is in
/// `skip_leaves`, any path in `excluded_dirs`, and any directory for
/// which `dir_skip` returns `true` (called with the absolute directory
/// path).
pub(crate) fn walk_skipping<'a, F>(
    root: &'a std::path::Path,
    excluded_dirs: &'a [PathBuf],
    skip_leaves: &'a [&'a str],
    dir_skip: F,
) -> impl Iterator<Item = std::io::Result<PathBuf>> + 'a
where
    F: Fn(&std::path::Path) -> bool + 'a,
{
    let mut stack = vec![root.to_path_buf()];
    std::iter::from_fn(move || loop {
        let next = stack.pop()?;
        if next.is_dir() {
            let leaf = next.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if skip_leaves.contains(&leaf) {
                continue;
            }
            if excluded_dirs.iter().any(|d| &next == d) {
                continue;
            }
            if dir_skip(&next) {
                continue;
            }
            match std::fs::read_dir(&next) {
                Ok(rd) => {
                    for entry in rd.flatten() {
                        stack.push(entry.path());
                    }
                }
                Err(e) => return Some(Err(e)),
            }
        } else {
            return Some(Ok(next));
        }
    })
}

/// Workspace-aware depth-first walker. Skips the same leaves as
/// [`walk_skipping`] plus any directory whose contents would match
/// `workspace.is_workspace_excluded` OR `workspace.is_excluded(language, …)`.
/// Recursion is pruned BEFORE `read_dir`, so populated `.venv/` or
/// `target/` cost zero IO.
///
/// Patterns like `.venv/**` describe directory CONTENTS, not the
/// directory itself, so we probe by appending `/x` to the relative
/// path before matching — if a synthetic child path matches, the dir
/// is excluded.
pub(crate) fn walk_for_language(
    workspace: &Workspace,
    language: kenn_model::Language,
) -> impl Iterator<Item = std::io::Result<PathBuf>> + '_ {
    let root = workspace.root();
    walk_skipping(
        root,
        workspace.excluded_dirs(),
        &["bin", "obj", ".git"],
        move |dir| {
            let Ok(rel) = dir.strip_prefix(root) else {
                return false;
            };
            let rel_str = rel.to_string_lossy();
            if rel_str.is_empty() {
                return false;
            }
            // Probe with a synthetic child so patterns of the form
            // `<dir>/**` (which describe contents) fire on the dir
            // itself.
            let probe = format!("{rel_str}/__kenn_probe__");
            workspace.is_workspace_excluded(&probe) || workspace.is_excluded(language, &probe)
        },
    )
}
