//! `[language.text]` — generic text-fallback indexing config.
//!
//! The fallback makes user-selected non-semantic text files (yaml/json/txt/…)
//! searchable when no semantic or native producer handles them. Unlike the
//! per-language blocks it is not extension-scoped: `include` is an explicit list
//! of file globs (there is no default — an empty list indexes nothing), so the
//! user decides exactly what the fallback reaches. `excludes` prune the
//! discovered set; both are raw pattern strings the text walker compiles to
//! `GlobSet`s at discovery time. `target_chars` / `overlap_chars` size the
//! recursive splitter.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextConfig {
    /// Disabled by default — opt in via `[language.text] enabled = true`.
    #[serde(default)]
    pub enabled: bool,
    /// File globs to fallback-index (e.g. `["**/*.yaml", "config/*.json"]`).
    /// **Empty by default** — explicit opt-in, since the fallback is a
    /// blast-radius-controlled catch-all, not "index everything".
    #[serde(default)]
    pub include: Vec<String>,
    /// Workspace-relative glob patterns removed from the discovered set.
    /// User-supplied values REPLACE the default fully (`excludes = []` opts
    /// out completely).
    #[serde(default = "default_text_excludes")]
    pub excludes: Vec<String>,
    /// Target chunk size in bytes for the recursive splitter. A file at or
    /// below this size is a single chunk.
    #[serde(default = "default_target_chars")]
    pub target_chars: usize,
    /// Bytes of trailing context re-included at the start of the next chunk
    /// (best-effort — never applied if it would not advance past the split).
    #[serde(default = "default_overlap_chars")]
    pub overlap_chars: usize,
}

impl TextConfig {
    /// Discovery exclude defaults — vendored / build / VCS noise that should
    /// never be fallback-indexed. `.kenn/**` is kenn's own derived store.
    pub const DEFAULT_EXCLUDES: &'static [&'static str] = &[
        "**/.git/**",
        "**/.kenn/**",
        "**/node_modules/**",
        "**/target/**",
    ];
}

fn default_text_excludes() -> Vec<String> {
    TextConfig::DEFAULT_EXCLUDES
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

const fn default_target_chars() -> usize {
    1000
}

const fn default_overlap_chars() -> usize {
    150
}

impl Default for TextConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            include: Vec::new(),
            excludes: default_text_excludes(),
            target_chars: default_target_chars(),
            overlap_chars: default_overlap_chars(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_disabled_with_no_include_and_standard_sizes() {
        let c = TextConfig::default();
        assert!(!c.enabled);
        assert!(c.include.is_empty());
        assert_eq!(c.excludes, TextConfig::DEFAULT_EXCLUDES);
        assert_eq!(c.target_chars, 1000);
        assert_eq!(c.overlap_chars, 150);
    }

    #[test]
    fn parses_include_excludes_and_sizes() {
        let toml = r#"
            enabled = true
            include = ["**/*.yaml", "config/*.json"]
            excludes = ["fixtures/**"]
            target_chars = 2000
            overlap_chars = 100
        "#;
        let c: TextConfig = toml::from_str(toml).unwrap();
        assert!(c.enabled);
        assert_eq!(c.include, ["**/*.yaml", "config/*.json"]);
        assert_eq!(c.excludes, vec!["fixtures/**".to_string()]);
        assert_eq!(c.target_chars, 2000);
        assert_eq!(c.overlap_chars, 100);
    }
}
