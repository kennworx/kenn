//! `[language.markdown]` — markdown corpus indexing config.
//!
//! Markdown has no external indexer and no SCIP step: it is walked by a
//! dedicated in-process producer. Roots are search globs over files/dirs (a
//! glob naming a directory means every `.md` beneath it, recursively);
//! `excludes` are glob patterns removed from the discovered set. Both are raw
//! pattern strings here — the markdown walker compiles them to `GlobSet`s at
//! discovery time.

use serde::{Deserialize, Serialize};

/// One configured markdown search root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkdownRoot {
    /// Search glob over a file or directory. A directory means recursive
    /// `.md` discovery beneath it (`<dir>/**/*.md`).
    pub glob: String,
    /// Label used in node identity (`md:<label>/<relpath>`). When omitted the
    /// walker uses `workspace` for an in-repo root; an external vault SHOULD
    /// set a distinct label so its ids stay disjoint from the repo's.
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkdownConfig {
    /// Disabled by default — opt in via `[language.markdown] enabled = true`.
    #[serde(default)]
    pub enabled: bool,
    /// Search roots. Default: a single in-repo root (`.`) scanned recursively
    /// for `.md`. User-supplied values REPLACE the default fully.
    #[serde(default = "default_markdown_roots")]
    pub roots: Vec<MarkdownRoot>,
    /// EXTRA markdown-specific exclude globs, on top of the ones the walk always
    /// inherits. The markdown walk spans the whole tree, so the caller
    /// (`markdown_with_inherited_excludes`) merges in the workspace-internal
    /// excludes (the git dir, kenn's store dir, `[workspace].excludes`) and every
    /// language's build/vendor excludes — this crate hardcodes none of them. This
    /// field only ADDS to that set; to index *under* an otherwise-excluded dir,
    /// use `includes`.
    #[serde(default)]
    pub excludes: Vec<String>,
    /// Re-include globs. A file matching one of these is indexed even when an
    /// inherited exclude would skip it — e.g. generated docs committed under a
    /// build dir. Takes precedence over `excludes`.
    #[serde(default)]
    pub includes: Vec<String>,
}

fn default_markdown_roots() -> Vec<MarkdownRoot> {
    vec![MarkdownRoot {
        glob: ".".into(),
        label: None,
    }]
}

impl Default for MarkdownConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            roots: default_markdown_roots(),
            excludes: Vec::new(),
            includes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_disabled_with_one_inrepo_root() {
        let c = MarkdownConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.roots.len(), 1);
        assert_eq!(c.roots[0].glob, ".");
        assert!(c.roots[0].label.is_none());
        assert!(c.excludes.is_empty()); // inherited at wire-up, never hardcoded here
        assert!(c.includes.is_empty());
    }

    #[test]
    fn parses_inrepo_and_labeled_external_roots() {
        let toml = r#"
            enabled = true
            excludes = ["drafts/**"]
            [[roots]]
            glob = "docs"
            [[roots]]
            glob = "/vaults/notes"
            label = "notes"
        "#;
        let c: MarkdownConfig = toml::from_str(toml).unwrap();
        assert!(c.enabled);
        assert_eq!(c.roots.len(), 2);
        assert_eq!(c.roots[0].glob, "docs");
        assert!(c.roots[0].label.is_none());
        assert_eq!(c.roots[1].label.as_deref(), Some("notes"));
        assert_eq!(c.excludes, vec!["drafts/**".to_string()]);
    }
}
