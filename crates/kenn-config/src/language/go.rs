//! `[language.go]` — scip-go SCIP indexer config.

use serde::{Deserialize, Serialize};

use super::Runtime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoConfig {
    /// Disabled by default — opt in via `[language.go] enabled = true`.
    /// scip-go needs a buildable Go module with a warm build cache, so
    /// we don't want to surprise users who only have a stray `go.mod`.
    #[serde(default)]
    pub enabled: bool,
    /// Launcher tokens. Default `["scip-go"]` — PATH lookup. Set to e.g.
    /// `["/opt/go/bin/scip-go"]` for an absolute path.
    #[serde(default = "default_go_command")]
    pub command: Vec<String>,
    /// Indexer runtime: `"local"` (default, host `PATH`) or `"docker"` (run
    /// `command` inside `image`). See the `docker-indexer-runtime` change.
    #[serde(default)]
    pub runtime: Runtime,
    /// OCI image used when `runtime = "docker"` — required then, ignored
    /// otherwise (validated in `Config::validate`).
    #[serde(default)]
    pub image: Option<String>,
    /// Workspace-relative glob patterns excluded from Go discovery AND
    /// ingest. Scoped to the Go pipeline only. User-supplied values
    /// REPLACE the default fully (`excludes = []` opts out completely).
    /// `vendor/` holds vendored dependency source and `testdata/` holds
    /// fixtures (often deliberately non-buildable) — a `go.mod` under
    /// either must not become its own module unit.
    #[serde(default = "default_go_excludes")]
    pub excludes: Vec<String>,
}

impl GoConfig {
    /// Workspace-walk exclude defaults specific to Go. Used as the serde
    /// default for `excludes` and as the documented constant.
    pub const DEFAULT_EXCLUDES: &'static [&'static str] =
        &["vendor/**", "**/vendor/**", "**/testdata/**"];
}

fn default_go_command() -> Vec<String> {
    vec!["scip-go".into()]
}

fn default_go_excludes() -> Vec<String> {
    GoConfig::DEFAULT_EXCLUDES
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

impl Default for GoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            command: default_go_command(),
            runtime: Runtime::Local,
            image: None,
            excludes: default_go_excludes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GoConfig;

    #[test]
    fn defaults_are_opt_in_with_scip_go_launcher() {
        let c = GoConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.command, vec!["scip-go".to_string()]);
        assert_eq!(
            c.excludes,
            vec![
                "vendor/**".to_string(),
                "**/vendor/**".to_string(),
                "**/testdata/**".to_string()
            ]
        );
    }

    #[test]
    fn excludes_can_be_opted_out() {
        let c: GoConfig = toml::from_str("excludes = []").unwrap();
        assert!(c.excludes.is_empty());
    }

    #[test]
    fn unknown_field_is_rejected() {
        toml::from_str::<GoConfig>("bogus = true").unwrap_err();
    }
}
