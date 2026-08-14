//! `[language.sql]` — SQL corpus indexing config.
//!
//! `dialect` names the primary SQL dialect. Leaving it unset is the normal
//! case and usually the better one: the permissive cross-dialect parse accepts
//! more real statements than any specific dialect, including the dialect a
//! database is named after. Measured over a fixed statement set, the permissive
//! parse scored 13/16 against oracle 10/16, postgres 10/16, and mysql 11/16 —
//! specific dialects are *stricter*, not better informed. Setting one is an
//! escape hatch for the cases where it genuinely wins (T-SQL bracket quoting),
//! not the normal path.
//!
//! There is no auto-detection knob, deliberately. Detecting a workspace's
//! "true" dialect would lower coverage, and the indexer instead retries a failed
//! parse against every remaining dialect, which subsumes selection entirely.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SqlConfig {
    /// Disabled by default — opt in via `[language.sql] enabled = true`.
    #[serde(default)]
    pub enabled: bool,
    /// Sources to parse. Globs over files/dirs; a directory means recursive
    /// `.sql` beneath it. Default: `["."]` (the whole workspace).
    #[serde(default = "default_sql_roots")]
    pub roots: Vec<String>,
    /// Primary dialect by name. `None` ⇒ the permissive cross-dialect parse.
    /// An unrecognized name is a configuration error, never a silent fallback.
    #[serde(default)]
    pub dialect: Option<String>,
    /// Additional user exclude globs. [`Self::ALWAYS_EXCLUDE`] is always applied
    /// on top and is never replaceable.
    #[serde(default)]
    pub excludes: Vec<String>,
}

impl SqlConfig {
    /// Build-output / vendor / VCS denies applied regardless of user excludes.
    pub const ALWAYS_EXCLUDE: &'static [&'static str] = &[
        "**/.git/**",
        "**/.kenn/**",
        "**/node_modules/**",
        "**/target/**",
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
}

fn default_sql_roots() -> Vec<String> {
    vec![".".to_string()]
}

impl Default for SqlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            roots: default_sql_roots(),
            dialect: None,
            excludes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_disabled_with_no_dialect() {
        let c = SqlConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.roots, ["."]);
        assert!(
            c.dialect.is_none(),
            "unset dialect is the permissive default, not a named one"
        );
        assert!(c.excludes.is_empty());
    }

    #[test]
    fn build_output_denies_always_apply_even_with_user_excludes() {
        let c = SqlConfig {
            excludes: vec!["fixtures/**".to_string()],
            ..Default::default()
        };
        let eff = c.effective_excludes();
        assert!(eff.iter().any(|e| e == "**/target/**"));
        assert!(eff.iter().any(|e| e == "fixtures/**"));
    }

    #[test]
    fn a_named_dialect_round_trips() {
        let c: SqlConfig = toml::from_str("enabled = true\ndialect = \"mssql\"\n").unwrap();
        assert!(c.enabled);
        assert_eq!(c.dialect.as_deref(), Some("mssql"));
    }
}
