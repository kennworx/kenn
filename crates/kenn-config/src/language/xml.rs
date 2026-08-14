//! `[language.xml]` — XML structure indexing config.
//!
//! `extensions` defaults to `["xml"]` alone. Other XML-shaped extensions
//! (`.xsd`, `.xsl`, project files) belong to tooling that already produces
//! them, and claiming them by default would take files from an existing
//! producer — so they are opt-in.
//!
//! The excludes are load-bearing rather than hygiene. Measured on a real
//! repository, build and vendor directories held 10854 `.xml` files against 485
//! first-party ones: an unexcluded walk indexes 22× more noise than content.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XmlConfig {
    /// Disabled by default — opt in via `[language.xml] enabled = true`.
    #[serde(default)]
    pub enabled: bool,
    /// Sources to parse. Globs over files/dirs; a directory means a recursive
    /// walk for the claimed extensions beneath it. Default: `["."]`.
    #[serde(default = "default_xml_roots")]
    pub roots: Vec<String>,
    /// Extensions this producer claims, without the leading dot. Defaults to
    /// `["xml"]`; add others explicitly when no other producer owns them.
    #[serde(default = "default_xml_extensions")]
    pub extensions: Vec<String>,
    /// Additional user exclude globs. [`Self::ALWAYS_EXCLUDE`] is always applied
    /// on top and is never replaceable.
    #[serde(default)]
    pub excludes: Vec<String>,
}

impl XmlConfig {
    /// Build-output / vendor / VCS denies applied regardless of user excludes.
    /// `bin`, `obj`, and `packages` matter disproportionately here: they are
    /// where compiled and restored XML accumulates.
    pub const ALWAYS_EXCLUDE: &'static [&'static str] = &[
        "**/.git/**",
        "**/.kenn/**",
        "**/node_modules/**",
        "**/target/**",
        "**/bin/**",
        "**/obj/**",
        "**/packages/**",
    ];

    /// Effective exclude set: always-on denies merged with the user's globs.
    #[must_use]
    pub fn effective_excludes(&self) -> Vec<String> {
        Self::ALWAYS_EXCLUDE
            .iter()
            .map(|s| (*s).to_string())
            .chain(self.excludes.iter().cloned())
            .collect()
    }

    /// Claimed extensions, lowercased and stripped of any leading dot so a
    /// user writing `".xsd"` and one writing `"xsd"` mean the same thing.
    #[must_use]
    pub fn claimed_extensions(&self) -> Vec<String> {
        self.extensions
            .iter()
            .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
            .collect()
    }
}

fn default_xml_roots() -> Vec<String> {
    vec![".".to_string()]
}

fn default_xml_extensions() -> Vec<String> {
    vec!["xml".to_string()]
}

impl Default for XmlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            roots: default_xml_roots(),
            extensions: default_xml_extensions(),
            excludes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_disabled_and_claim_only_xml() {
        let c = XmlConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.roots, ["."]);
        assert_eq!(
            c.claimed_extensions(),
            ["xml"],
            "other XML-shaped extensions belong to producers that own them"
        );
    }

    #[test]
    fn build_and_vendor_denies_always_apply() {
        let c = XmlConfig {
            excludes: vec!["fixtures/**".to_string()],
            ..Default::default()
        };
        let eff = c.effective_excludes();
        for deny in ["**/bin/**", "**/obj/**", "**/packages/**"] {
            assert!(eff.iter().any(|e| e == deny), "{deny} kept");
        }
        assert!(eff.iter().any(|e| e == "fixtures/**"));
    }

    #[test]
    fn a_configured_extra_extension_is_claimed_dot_insensitively() {
        let c: XmlConfig =
            toml::from_str("enabled = true\nextensions = [\"xml\", \".xsd\"]\n").unwrap();
        assert_eq!(c.claimed_extensions(), ["xml", "xsd"]);
    }
}
