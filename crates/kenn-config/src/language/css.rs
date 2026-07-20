//! `[language.css]` — stylesheet (CSS + Sass) corpus indexing config.
//!
//! One config covers both `.css` and Sass sources even though the internal
//! `Language` is split `css`/`sass`: `roots` (what to parse), `usage_sources`
//! (what to scan for class usage), and `excludes` are shared; only Sass-only
//! knobs live in the nested `sass` subsection. The build-output/vendor denies in
//! [`CssConfig::ALWAYS_EXCLUDE`] are always applied on top of the user's
//! `excludes` (never replaceable) so compiled CSS can't be silently re-admitted
//! alongside its Sass source.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Sass-only settings; ignored for `.css`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SassConfig {
    /// Override path to the `sass` compiler. `None` ⇒ auto-discover.
    #[serde(default)]
    pub compiler: Option<PathBuf>,
    /// `@use`/`@import` load-path roots passed to dart-sass.
    #[serde(default)]
    pub load_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CssConfig {
    /// Disabled by default — opt in via `[language.css] enabled = true`.
    #[serde(default)]
    pub enabled: bool,
    /// Stylesheet sources to parse (the registry). Globs over files/dirs; a
    /// directory means recursive `.css`/`.scss`/`.sass` beneath it. Default:
    /// `["."]` (the whole workspace).
    #[serde(default = "default_css_roots")]
    pub roots: Vec<String>,
    /// Files to scan for class usage (Phase 2), as raw-text globs. **Empty by
    /// default** — explicit opt-in, since not every project in a repo uses CSS.
    #[serde(default)]
    pub usage_sources: Vec<String>,
    /// Additional user exclude globs. [`Self::ALWAYS_EXCLUDE`] is always applied
    /// on top (never replaceable) — see [`Self::effective_excludes`].
    #[serde(default)]
    pub excludes: Vec<String>,
    /// Sass-only settings.
    #[serde(default)]
    pub sass: SassConfig,
}

impl CssConfig {
    /// Build-output / vendor / VCS denies that are ALWAYS excluded, regardless
    /// of user `excludes`. Indexing compiled CSS (e.g. `dist/`) alongside its
    /// Sass source would double-count classes, so these are not replaceable.
    pub const ALWAYS_EXCLUDE: &'static [&'static str] = &[
        "**/.git/**",
        "**/.kenn/**",
        "**/node_modules/**",
        "**/target/**",
        "**/dist/**",
        "**/build/**",
    ];

    /// Effective exclude set: the always-on build/vendor denies merged with the
    /// user's additional `excludes`.
    #[must_use]
    pub fn effective_excludes(&self) -> Vec<String> {
        Self::ALWAYS_EXCLUDE
            .iter()
            .map(|s| (*s).to_string())
            .chain(self.excludes.iter().cloned())
            .collect()
    }
}

fn default_css_roots() -> Vec<String> {
    vec![".".to_string()]
}

impl Default for CssConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            roots: default_css_roots(),
            usage_sources: Vec::new(),
            excludes: Vec::new(),
            sass: SassConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_disabled_with_workspace_root_and_no_usage() {
        let c = CssConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.roots, ["."]);
        assert!(c.usage_sources.is_empty());
        assert!(c.excludes.is_empty());
        assert!(c.sass.compiler.is_none());
    }

    #[test]
    fn build_output_denies_always_apply_even_with_user_excludes() {
        let c = CssConfig {
            excludes: vec!["fixtures/**".to_string()],
            ..Default::default()
        };
        let eff = c.effective_excludes();
        assert!(eff.iter().any(|e| e == "**/dist/**"), "dist deny kept");
        assert!(eff.iter().any(|e| e == "**/build/**"), "build deny kept");
        assert!(
            eff.iter().any(|e| e == "fixtures/**"),
            "user exclude merged"
        );
    }

    #[test]
    fn parses_roots_usage_sources_and_nested_sass() {
        let c: CssConfig = toml::from_str(
            r#"
                enabled = true
                roots = ["src/**/*.{css,scss}"]
                usage_sources = ["src/**/*.tsx"]
                excludes = ["legacy/**"]
                [sass]
                compiler = "node_modules/.bin/sass"
                load_paths = ["node_modules"]
            "#,
        )
        .unwrap();
        assert!(c.enabled);
        assert_eq!(c.roots, ["src/**/*.{css,scss}"]);
        assert_eq!(c.usage_sources, ["src/**/*.tsx"]);
        assert_eq!(c.sass.load_paths, ["node_modules"]);
        assert_eq!(
            c.sass.compiler.as_deref(),
            Some(std::path::Path::new("node_modules/.bin/sass"))
        );
    }
}
