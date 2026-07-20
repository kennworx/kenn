//! `[vectors]` section — relocates the committed vector sidecar root.
//!
//! The committed code + findings vector sidecars normally live under
//! `<source_root>/.kenn/vectors/{code,findings}/`. Setting `location`
//! moves the `vectors/` parent to a sibling directory — typical use is
//! a Syncthing/Dropbox/NAS folder shared across a team, or a per-user
//! XDG cache shared across worktrees of the same repo.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorsConfig {
    /// Override for the committed vectors root. Same value space as
    /// `[layout] derived_root`: a relative path (resolved from the
    /// repository's main worktree, falling back to the source root
    /// outside a git tree), an absolute path, or the keyword `"global"`
    /// (an XDG-cache path keyed by a stable per-repository project id,
    /// shared across the repo's branches and worktrees). Defaults to
    /// `<main worktree>/.kenn/vectors` when unset.
    #[serde(default)]
    pub location: Option<String>,
    /// Size cap (MiB) for the multi-generation vector cache. When the
    /// cache exceeds the cap, garbage collection evicts least-recently-
    /// used generation directories (never the active generation, never
    /// one holding committed `pack-*.bin` files). `0` disables GC.
    #[serde(default = "default_cache_cap_mb")]
    pub cache_cap_mb: u64,
}

impl Default for VectorsConfig {
    fn default() -> Self {
        Self {
            location: None,
            cache_cap_mb: default_cache_cap_mb(),
        }
    }
}

fn default_cache_cap_mb() -> u64 {
    1024
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    #[test]
    fn defaults_to_no_location() {
        let c = Config::from_toml("").unwrap();
        assert!(c.vectors.location.is_none());
        assert_eq!(c.vectors.cache_cap_mb, 1024);
    }

    #[test]
    fn parses_cache_cap() {
        let c = Config::from_toml("[vectors]\ncache_cap_mb = 0\n").unwrap();
        assert_eq!(c.vectors.cache_cap_mb, 0);
    }

    #[test]
    fn parses_location() {
        let c = Config::from_toml("[vectors]\nlocation = \"global\"\n").unwrap();
        assert_eq!(c.vectors.location.as_deref(), Some("global"));
        let c = Config::from_toml("[vectors]\nlocation = \"/mnt/shared/vectors\"\n").unwrap();
        assert_eq!(c.vectors.location.as_deref(), Some("/mnt/shared/vectors"));
        let c = Config::from_toml("[vectors]\nlocation = \"team-vectors\"\n").unwrap();
        assert_eq!(c.vectors.location.as_deref(), Some("team-vectors"));
    }
}
