//! `[layout]` section — the config-driven store layout (`store-layout`).
//!
//! Only the *derived* store root is configurable. The committed root —
//! the git-tracked `vectors/` and `findings/` sidecars — is always
//! `<source_root>/.kenn` and deliberately not a config knob: a settable
//! committed root could point version-controlled embeddings out of the
//! repository.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutConfig {
    /// Root for the derived, throwaway, gitignored store — the code
    /// graph, `snapshots/`, `live`, `building/`, `index.lock`, and the
    /// `scip-*.scip` indexer intermediates. Accepts a relative path
    /// (resolved from the source root), an absolute path, or the keyword
    /// `"global"` — an XDG-cache path keyed by a stable per-repository
    /// project id, shared across the repo's branches and worktrees.
    /// Defaults to `<source_root>/.kenn/local` when unset.
    #[serde(default)]
    pub derived_root: Option<String>,
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    #[test]
    fn defaults_to_no_derived_root() {
        let c = Config::from_toml("").unwrap();
        assert!(c.layout.derived_root.is_none());
    }

    #[test]
    fn parses_derived_root() {
        let c = Config::from_toml("[layout]\nderived_root = \"global\"\n").unwrap();
        assert_eq!(c.layout.derived_root.as_deref(), Some("global"));
        let c = Config::from_toml("[layout]\nderived_root = \"/var/cache/kenn\"\n").unwrap();
        assert_eq!(c.layout.derived_root.as_deref(), Some("/var/cache/kenn"));
    }
}
