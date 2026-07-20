//! `[workspace]` section — source root and cross-language excludes.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    /// Source root — where the code to index lives. Defaults to the
    /// current working directory when None. The committed store
    /// (`.kenn/`) always sits under this root; relocate only the
    /// *derived* store, via `[layout] derived_root`.
    #[serde(default)]
    pub root: Option<PathBuf>,
    /// Workspace-wide additional exclude patterns. The runtime ALSO
    /// hardcodes `.git/**` and `**/.git/**` plus the auto-discovered
    /// linked git worktrees — those skips are kenn invariants, not
    /// configurable. Per-language conventions like `target/**`,
    /// `__pycache__/**` live on `[language.X].excludes` instead and
    /// are scoped to that language's pipeline only.
    #[serde(default)]
    pub excludes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    #[test]
    fn workspace_excludes_parses() {
        let toml = "[workspace]\nexcludes = [\"sensitive/**\"]\n";
        let c = Config::from_toml(toml).unwrap();
        assert_eq!(c.workspace.excludes, vec!["sensitive/**".to_string()]);
    }

    #[test]
    fn workspace_excludes_default_empty() {
        let c = Config::from_toml("").unwrap();
        assert!(c.workspace.excludes.is_empty());
    }
}
