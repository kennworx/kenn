//! `[language.html]` — HTML corpus indexing config.
//!
//! Mirrors [`CssConfig`](super::css::CssConfig): `roots` (what to parse) and
//! `excludes` (user denies), with the build-output/vendor denies in
//! [`HtmlConfig::ALWAYS_EXCLUDE`] always merged on top (never replaceable) so
//! generated HTML under `dist/`/`build/` can't be silently re-admitted.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HtmlConfig {
    /// Disabled by default — opt in via `[language.html] enabled = true`.
    #[serde(default)]
    pub enabled: bool,
    /// HTML sources to parse. Globs over files/dirs; a directory means
    /// recursive `.html`/`.htm` beneath it. Default: `["."]` (the workspace).
    #[serde(default = "default_html_roots")]
    pub roots: Vec<String>,
    /// Additional user exclude globs. [`Self::ALWAYS_EXCLUDE`] is always applied
    /// on top (never replaceable) — see [`Self::effective_excludes`].
    #[serde(default)]
    pub excludes: Vec<String>,
}

impl HtmlConfig {
    /// Build-output / vendor / VCS denies that are ALWAYS excluded, regardless
    /// of user `excludes`.
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

fn default_html_roots() -> Vec<String> {
    vec![".".to_string()]
}

impl Default for HtmlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            roots: default_html_roots(),
            excludes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_disabled_with_workspace_root() {
        let c = HtmlConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.roots, ["."]);
        assert!(c.excludes.is_empty());
    }

    #[test]
    fn build_output_denies_always_apply_even_with_user_excludes() {
        let c = HtmlConfig {
            excludes: vec!["fixtures/**".to_string()],
            ..Default::default()
        };
        let eff = c.effective_excludes();
        assert!(eff.iter().any(|e| e == "**/dist/**"), "dist deny kept");
        assert!(
            eff.iter().any(|e| e == "fixtures/**"),
            "user exclude merged"
        );
    }

    #[test]
    fn parses_roots_and_excludes() {
        let c: HtmlConfig = toml::from_str(
            r#"
                enabled = true
                roots = ["pages/**/*.html"]
                excludes = ["legacy/**"]
            "#,
        )
        .unwrap();
        assert!(c.enabled);
        assert_eq!(c.roots, ["pages/**/*.html"]);
        assert_eq!(c.excludes, ["legacy/**"]);
    }
}
