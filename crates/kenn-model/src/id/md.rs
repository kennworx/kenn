//! Markdown public-ID construction and GitHub-style heading slugification.
//!
//! Markdown nodes are not produced from SCIP; their native IDs are built
//! directly from the corpus root label, the file's workspace-relative path,
//! and (for sections) a slugified heading. The public ID is
//! `md:<root>/<relpath>` for a `document` node and
//! `md:<root>/<relpath>#<slug>` for a `section`.

use std::collections::HashMap;

use crate::id::PublicId;
use crate::language::Language;

/// Public ID of a markdown corpus module (`module` kind) — a root or one of
/// its sub-directories. `dir` is the `/`-joined directory path relative to the
/// root (empty for the root module itself): `md:<root>` for the root,
/// `md:<root>/<dir>` for a sub-directory. A document's `enclosing` module is
/// the module for the directory the file sits in.
#[must_use]
pub fn module_id(root: &str, dir: &str) -> PublicId {
    let native = if dir.is_empty() {
        root.to_string()
    } else {
        format!("{root}/{dir}")
    };
    PublicId::new(Language::Markdown, &native)
}

/// The ancestor module directory chain for a file `relpath`, root-first and
/// inclusive of the root module (`""`). For `a/b/x.md` → `["", "a", "a/b"]`,
/// i.e. the modules `md:<root>`, `md:<root>/a`, `md:<root>/a/b`; the last is the
/// document's immediate enclosing module. A root-level file → `[""]`.
#[must_use]
pub fn module_chain(relpath: &str) -> Vec<String> {
    let mut chain = vec![String::new()];
    let Some((dir, _file)) = relpath.rsplit_once('/') else {
        return chain; // root-level file: only the root module
    };
    let mut acc = String::new();
    for seg in dir.split('/') {
        if acc.is_empty() {
            acc.push_str(seg);
        } else {
            acc.push('/');
            acc.push_str(seg);
        }
        chain.push(acc.clone());
    }
    chain
}

/// Public ID of a markdown file-as-node (`document` kind).
#[must_use]
pub fn document_id(root: &str, relpath: &str) -> PublicId {
    PublicId::new(Language::Markdown, &format!("{root}/{relpath}"))
}

/// Public ID of a markdown section (`section` kind).
#[must_use]
pub fn section_id(root: &str, relpath: &str, slug: &str) -> PublicId {
    PublicId::new(Language::Markdown, &format!("{root}/{relpath}#{slug}"))
}

/// GitHub-style heading slug: lowercase; keep letters, digits, and
/// underscores; whitespace becomes `-`; existing `-` is kept; all other
/// punctuation is dropped. Multiple hyphens are not collapsed (matching
/// GitHub / `github-slugger`).
#[must_use]
pub fn slugify(heading: &str) -> String {
    let mut out = String::with_capacity(heading.len());
    for ch in heading.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if ch == '_' || ch == '-' {
            out.push(ch);
        } else if ch.is_whitespace() {
            out.push('-');
        }
        // every other character is dropped
    }
    out
}

/// In-file slug disambiguator. The first occurrence of a slug is returned
/// bare; the Nth repeat gets a `-{N-1}` suffix (`foo`, `foo-1`, `foo-2`),
/// matching GitHub's within-document heading anchors.
#[derive(Debug, Default)]
pub struct SlugDeduper {
    seen: HashMap<String, u32>,
}

impl SlugDeduper {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a unique slug for `slug` within this file, suffixing repeats.
    pub fn dedup(&mut self, slug: &str) -> String {
        let count = self.seen.entry(slug.to_string()).or_insert(0);
        let result = if *count == 0 {
            slug.to_string()
        } else {
            format!("{slug}-{count}")
        };
        *count += 1;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_and_section_ids_carry_root_and_anchor() {
        assert_eq!(
            document_id("workspace", "docs/auth.md").as_str(),
            "md:workspace/docs/auth.md"
        );
        assert_eq!(
            section_id("workspace", "docs/auth.md", "flow").as_str(),
            "md:workspace/docs/auth.md#flow"
        );
    }

    #[test]
    fn module_ids_and_chain() {
        assert_eq!(module_id("workspace", "").as_str(), "md:workspace");
        assert_eq!(
            module_id("workspace", "docs/a").as_str(),
            "md:workspace/docs/a"
        );
        // The chain is root-first, inclusive of the root, ending at the file's
        // immediate directory module.
        assert_eq!(module_chain("docs/a/x.md"), ["", "docs", "docs/a"]);
        assert_eq!(module_chain("x.md"), [""]); // root-level file
    }

    #[test]
    fn two_roots_same_relpath_are_distinct() {
        assert_ne!(
            document_id("workspace", "notes/x.md").as_str(),
            document_id("vault", "notes/x.md").as_str()
        );
    }

    #[test]
    fn slugify_matches_github_shape() {
        assert_eq!(slugify("Flow Login"), "flow-login");
        assert_eq!(slugify("What's New?"), "whats-new");
        assert_eq!(slugify("snake_case-kept"), "snake_case-kept");
        assert_eq!(slugify("Café Münü"), "café-münü");
    }

    #[test]
    fn duplicate_headings_get_numeric_suffixes() {
        let mut d = SlugDeduper::new();
        assert_eq!(d.dedup("notes"), "notes");
        assert_eq!(d.dedup("notes"), "notes-1");
        assert_eq!(d.dedup("notes"), "notes-2");
        assert_eq!(d.dedup("other"), "other");
    }
}
