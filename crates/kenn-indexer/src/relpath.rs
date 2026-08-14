//! The one rule for turning a link-relative path into a workspace-relative one.
//!
//! A link target — a markdown `[t](../foo/bar.md)`, an HTML `<a href>`, a
//! reference to a source file — is written relative to the **linking** file, so
//! it must be joined onto that file's directory and normalized before it can be
//! compared to a candidate's workspace-relative path.
//!
//! This lived in three places before `honest-link-grades`: `markdown::resolve`
//! and `html::links::core` each had a correct copy, while `markdown::code_resolve`
//! had one that "resolved" `..` by *deleting the token*
//! (`path.replace("../", "")`) — and that was the copy on the shared grading
//! path for md→code and HTML→file links. Deleting a `..` yields a different
//! path, so a correct link missed its exact match and degraded to `drifted`,
//! and — because file candidates are pre-filtered by basename — a link could be
//! graded `exact` against the *wrong* same-named file. One rule, one
//! implementation, no room for a fourth.

/// Join `target`, written relative to `linking_relpath`, into a
/// workspace-relative path. Returns `None` when `..` segments walk above the
/// workspace root — the target then is not in the corpus, and callers must not
/// resolve it against the root.
///
/// A `target` starting with `/` is workspace-root-relative (the convention a
/// forge renders for `[t](/docs/x.md)` and `<a href="/index.html">`), so it
/// ignores the linking file's directory.
///
/// Pure string math — no filesystem, `/`-normalized paths only.
#[must_use]
pub fn join_relative(linking_relpath: &str, target: &str) -> Option<String> {
    // Only `/` separates segments here, so a `\`-separated or drive-absolute
    // target would survive as a single opaque segment and defeat the
    // above-root guard below. That matters because the joined path is handed to
    // `Path::join`, which on Windows honours both — `..\..\secrets` would walk
    // out of the workspace, and `C:/x` would replace the base entirely. Neither
    // is a legal CommonMark-relative destination, so reject rather than
    // normalize.
    if target.contains('\\') || is_drive_absolute(target) {
        return None;
    }
    let mut segs: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        let dir = linking_relpath.rsplit_once('/').map_or("", |(d, _)| d);
        if dir.is_empty() {
            Vec::new()
        } else {
            dir.split('/').collect()
        }
    };
    for seg in target.trim_start_matches('/').split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segs.pop()?; // escapes the workspace root → not in-corpus
            }
            s => segs.push(s),
        }
    }
    Some(segs.join("/"))
}

/// A `X:`-prefixed target (`C:/Windows`, `c:foo`). `Path::join` treats such a
/// component as absolute and discards the base, so it can never be
/// workspace-relative.
fn is_drive_absolute(target: &str) -> bool {
    let mut chars = target.chars();
    matches!((chars.next(), chars.next()), (Some(c), Some(':')) if c.is_ascii_alphabetic())
}

/// Whether a target exists in the workspace at a canonical workspace-relative
/// path, for link targets that resolved to no graph node. A link to a real file
/// or directory kenn does not index (`LICENSE-MIT`, `docs/`, `logo.png`) is not
/// broken, and reporting it as such was the defect `honest-link-grades`
/// removed.
///
/// Injected rather than performed inline so the resolvers stay filesystem-free
/// and unit-testable: the caller decides the backing (the real filesystem in
/// the ingest paths, a fixed set in tests). Both the markdown and HTML corpora
/// resolve through this one trait — the HTML half shipped first as
/// `AssetIndex`, and markdown having no equivalent is why an existing
/// `LICENSE-MIT` dangled.
pub trait PathExists {
    /// True when the workspace holds a file **or directory** at this canonical
    /// workspace-relative path. Directories count: a link to one points at
    /// something real.
    fn exists(&self, canonical_path: &str) -> bool;
}

/// [`PathExists`] over the real workspace — the backing every ingest path uses.
/// A target counts as existing when the workspace root holds a file **or
/// directory** at its canonical path, so a directory link (`docs/`,
/// `../kenn-model`) resolves rather than dangling.
pub struct FsPaths<'a> {
    pub workspace_root: &'a std::path::Path,
}

impl PathExists for FsPaths<'_> {
    fn exists(&self, canonical_path: &str) -> bool {
        !canonical_path.is_empty() && self.workspace_root.join(canonical_path).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::join_relative;

    #[test]
    fn joins_against_the_linking_files_directory() {
        assert_eq!(
            join_relative("indexers/kenn-dotnet/README.md", "../frames.ts").as_deref(),
            Some("indexers/frames.ts")
        );
    }

    #[test]
    fn pops_one_segment_per_parent_hop_rather_than_deleting_the_token() {
        // The bug this module exists to remove: `replace("../", "")` would give
        // `x/mod.rs` here, which is a *different* file from the correct answer.
        assert_eq!(
            join_relative("crates/a/src/m/README.md", "../../x/mod.rs").as_deref(),
            Some("crates/a/x/mod.rs")
        );
    }

    #[test]
    fn walking_above_the_root_is_not_in_corpus() {
        assert_eq!(join_relative("a/b.md", "../../../x.md"), None);
    }

    #[test]
    fn a_root_relative_target_ignores_the_linking_directory() {
        assert_eq!(
            join_relative("docs/deep/a.md", "/README.md").as_deref(),
            Some("README.md")
        );
    }

    #[test]
    fn dot_segments_and_empty_segments_are_skipped() {
        assert_eq!(
            join_relative("docs/a.md", "./b//c.md").as_deref(),
            Some("docs/b/c.md")
        );
    }

    /// The project targets mac, linux and windows. A `\`-separated or
    /// drive-absolute destination would pass the `..`-popping guard as one
    /// opaque segment and then escape the workspace via `Path::join`.
    #[test]
    fn windows_shaped_targets_are_not_in_corpus() {
        assert_eq!(join_relative("a/b.md", r"..\..\..\secrets"), None);
        assert_eq!(join_relative("a/b.md", "C:/Windows/System32"), None);
        assert_eq!(join_relative("a/b.md", "c:notes"), None);
        // A colon that is not a drive letter is still an ordinary segment.
        assert_eq!(
            join_relative("a/b.md", "notes:2026.md").as_deref(),
            Some("a/notes:2026.md")
        );
    }

    #[test]
    fn a_file_at_the_root_joins_from_the_root() {
        assert_eq!(
            join_relative("README.md", "docs/x.md").as_deref(),
            Some("docs/x.md")
        );
    }
}
