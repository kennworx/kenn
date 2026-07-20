//! The global markdown resolution index.
//!
//! Built from the phase-1 collect of every discovered file (design D4): it
//! maps the keys a link can name — relative path, filename stem, frontmatter
//! alias, title — to candidate `document` node ids, and records each
//! document's section slugs for `#anchor` resolution. Resolution (the ladder)
//! queries this index; building it is purely additive and order-independent.
//!
//! A key may map to several candidates (e.g. two `auth.md` in different
//! directories); the resolver's locality / keep-all logic decides among them.

use std::collections::{HashMap, HashSet};

use kenn_model::id::md::document_id;

use super::collect::CollectedFile;
use super::discover::DiscoveredMarkdown;

/// A candidate node the resolver may pick, with the fields its tiebreaks need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRef {
    /// Public id of the `document` node (`md:<label>/<relpath>`).
    pub id: String,
    pub label: String,
    pub relpath: String,
}

#[derive(Debug, Default)]
pub struct ResolutionIndex {
    by_path: HashMap<String, Vec<NodeRef>>,
    by_stem: HashMap<String, Vec<NodeRef>>,
    by_alias: HashMap<String, Vec<NodeRef>>,
    by_title: HashMap<String, Vec<NodeRef>>,
    /// `document` node id → its section slugs (for `file#slug` resolution).
    slugs: HashMap<String, HashSet<String>>,
}

impl ResolutionIndex {
    /// Build the index from the collected corpus.
    pub fn build<'a, I>(files: I) -> Self
    where
        I: IntoIterator<Item = (&'a DiscoveredMarkdown, &'a CollectedFile)>,
    {
        let mut idx = Self::default();
        for (file, collected) in files {
            idx.add(file, collected);
        }
        idx
    }

    fn add(&mut self, file: &DiscoveredMarkdown, collected: &CollectedFile) {
        let node = NodeRef {
            id: document_id(&file.label, &file.relpath).into_string(),
            label: file.label.clone(),
            relpath: file.relpath.clone(),
        };

        // by_path: exact relpath and its extension-stripped form.
        push(&mut self.by_path, file.relpath.to_lowercase(), &node);
        if let Some(no_ext) = strip_md_ext(&file.relpath) {
            push(&mut self.by_path, no_ext.to_lowercase(), &node);
        }
        // by_stem: the filename without directory or extension.
        push(&mut self.by_stem, stem(&file.relpath), &node);
        // by_alias / by_title from frontmatter.
        for alias in &collected.frontmatter.aliases {
            push(&mut self.by_alias, alias.to_lowercase(), &node);
        }
        if let Some(title) = &collected.frontmatter.title {
            push(&mut self.by_title, title.to_lowercase(), &node);
        }
        // section slugs for #anchor resolution.
        let set: HashSet<String> = collected.headings.iter().map(|h| h.slug.clone()).collect();
        self.slugs.insert(node.id, set);
    }

    /// Candidates for an exact relative path (with or without `.md` extension).
    #[must_use]
    pub fn by_path(&self, path: &str) -> &[NodeRef] {
        self.by_path
            .get(&path.to_lowercase())
            .map_or(&[], Vec::as_slice)
    }

    /// Candidates for a bare filename stem (`[[auth]]`).
    #[must_use]
    pub fn by_stem(&self, name: &str) -> &[NodeRef] {
        self.by_stem
            .get(&name.to_lowercase())
            .map_or(&[], Vec::as_slice)
    }

    /// Candidates for a frontmatter alias.
    #[must_use]
    pub fn by_alias(&self, alias: &str) -> &[NodeRef] {
        self.by_alias
            .get(&alias.to_lowercase())
            .map_or(&[], Vec::as_slice)
    }

    /// Candidates for a frontmatter title.
    #[must_use]
    pub fn by_title(&self, title: &str) -> &[NodeRef] {
        self.by_title
            .get(&title.to_lowercase())
            .map_or(&[], Vec::as_slice)
    }

    /// Whether `slug` is a section of the document with id `node_id`.
    #[must_use]
    pub fn has_section(&self, node_id: &str, slug: &str) -> bool {
        self.slugs.get(node_id).is_some_and(|s| s.contains(slug))
    }
}

fn push(map: &mut HashMap<String, Vec<NodeRef>>, key: String, node: &NodeRef) {
    let bucket = map.entry(key).or_default();
    if !bucket.contains(node) {
        bucket.push(node.clone());
    }
}

fn strip_md_ext(relpath: &str) -> Option<&str> {
    relpath
        .strip_suffix(".md")
        .or_else(|| relpath.strip_suffix(".markdown"))
}

fn stem(relpath: &str) -> String {
    let file = relpath.rsplit('/').next().unwrap_or(relpath);
    strip_md_ext(file).unwrap_or(file).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::collect::{Frontmatter, HeadingSlug};
    use std::path::PathBuf;

    fn file(label: &str, relpath: &str) -> DiscoveredMarkdown {
        DiscoveredMarkdown {
            abs_path: PathBuf::from(relpath),
            label: label.into(),
            relpath: relpath.into(),
            in_repo: true,
        }
    }

    fn collected(title: Option<&str>, aliases: &[&str], slugs: &[&str]) -> CollectedFile {
        CollectedFile {
            frontmatter: Frontmatter {
                title: title.map(ToString::to_string),
                aliases: aliases.iter().map(|s| (*s).to_string()).collect(),
                tags: vec![],
                related: vec![],
            },
            headings: slugs
                .iter()
                .enumerate()
                .map(|(i, s)| HeadingSlug {
                    level: 2,
                    text: (*s).to_string(),
                    slug: (*s).to_string(),
                    line: u32::try_from(i + 1).unwrap_or(0),
                })
                .collect(),
        }
    }

    #[test]
    fn resolves_by_path_stem_alias_title() {
        let f = file("workspace", "docs/auth.md");
        let c = collected(
            Some("Authentication Flow"),
            &["login-flow"],
            &["overview", "flow"],
        );
        let idx = ResolutionIndex::build([(&f, &c)]);
        let want = "md:workspace/docs/auth.md";

        assert_eq!(idx.by_path("docs/auth.md")[0].id, want);
        assert_eq!(idx.by_path("docs/auth")[0].id, want); // extension-stripped
        assert_eq!(idx.by_stem("auth")[0].id, want);
        assert_eq!(idx.by_stem("AUTH")[0].id, want); // case-insensitive
        assert_eq!(idx.by_alias("login-flow")[0].id, want);
        assert_eq!(idx.by_title("authentication flow")[0].id, want);
        assert!(idx.has_section(want, "flow"));
        assert!(!idx.has_section(want, "missing"));
    }

    #[test]
    fn shared_stem_yields_multiple_candidates() {
        let f1 = file("workspace", "api/auth.md");
        let f2 = file("workspace", "ui/auth.md");
        let c = collected(None, &[], &[]);
        let idx = ResolutionIndex::build([(&f1, &c), (&f2, &c)]);
        let mut ids: Vec<&str> = idx.by_stem("auth").iter().map(|n| n.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["md:workspace/api/auth.md", "md:workspace/ui/auth.md"]);
        // but the exact path stays unambiguous
        assert_eq!(idx.by_path("api/auth.md").len(), 1);
    }

    #[test]
    fn unknown_keys_return_empty() {
        let idx = ResolutionIndex::default();
        assert!(idx.by_stem("nope").is_empty());
        assert!(!idx.has_section("md:workspace/x.md", "y"));
    }
}
