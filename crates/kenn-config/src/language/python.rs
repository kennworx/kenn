//! `[language.python]` — scip-python SCIP indexer config.

use serde::{Deserialize, Serialize};

use super::Runtime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonConfig {
    /// Disabled by default — opt in via `[language.python] enabled = true`.
    #[serde(default)]
    pub enabled: bool,
    /// Launcher tokens. Default `["scip-python"]` — PATH lookup. Common
    /// alternatives: `["bunx", "@sourcegraph/scip-python"]`,
    /// `["npx", "--yes", "@sourcegraph/scip-python"]`,
    /// `["uvx", "scip-python"]`.
    #[serde(default = "default_python_command")]
    pub command: Vec<String>,
    /// Indexer runtime: `"local"` (default, host `PATH`) or `"docker"` (run
    /// `command` inside `image`). See the `docker-indexer-runtime` change.
    #[serde(default)]
    pub runtime: Runtime,
    /// OCI image used when `runtime = "docker"` — required then, ignored
    /// otherwise (validated in `Config::validate`).
    #[serde(default)]
    pub image: Option<String>,
    /// Forwarded as `--project-name <name>` when set. Becomes the
    /// distribution segment of every scip-python symbol; defaults to
    /// empty in scip-python itself.
    #[serde(default)]
    pub project_name: Option<String>,
    /// Forwarded as `--project-version <ver>` when set. Defaults to the
    /// current git revision inside scip-python.
    #[serde(default)]
    pub project_version: Option<String>,
    /// Workspace-relative sub-package directories to index. Empty
    /// (default) runs one whole-workspace scip-python pass. Each entry
    /// produces a SEPARATE scip-python invocation with `--target-only
    /// <abs>`; scip-python cannot share Pyright state across runs, so
    /// N entries cost N × per-target analysis time. Absolute paths and
    /// duplicate entries are rejected at config load.
    #[serde(default)]
    pub targets: Vec<String>,
    /// Workspace-relative glob patterns excluded from Python discovery
    /// AND ingest. Scoped to the Python pipeline only. User-supplied
    /// values REPLACE the default fully (`excludes = []` opts out).
    /// Composes with `targets` (excludes filter what scip-python's
    /// `--target-only` walk would otherwise emit).
    #[serde(default = "default_python_excludes")]
    pub excludes: Vec<String>,
}

impl PythonConfig {
    /// Workspace-walk exclude defaults specific to Python: bytecode
    /// cache, virtualenv conventions, tox, setuptools / wheel build
    /// artefacts.
    pub const DEFAULT_EXCLUDES: &'static [&'static str] = &[
        "__pycache__/**",
        "**/__pycache__/**",
        ".venv/**",
        "**/.venv/**",
        "venv/**",
        "**/venv/**",
        ".tox/**",
        "**/.tox/**",
        "dist/**",
        "**/dist/**",
        "build/**",
        "**/build/**",
        "*.egg-info/**",
        "**/*.egg-info/**",
    ];
}

fn default_python_command() -> Vec<String> {
    vec!["scip-python".into()]
}

fn default_python_excludes() -> Vec<String> {
    PythonConfig::DEFAULT_EXCLUDES
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

impl Default for PythonConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            command: default_python_command(),
            runtime: Runtime::Local,
            image: None,
            project_name: None,
            project_version: None,
            targets: Vec::new(),
            excludes: default_python_excludes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::error::ConfigError;

    #[test]
    fn excludes_default_to_constant() {
        let c = Config::from_toml("[language.python]\nenabled = true\n").unwrap();
        assert_eq!(
            c.language.python.excludes,
            PythonConfig::DEFAULT_EXCLUDES
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn user_set_excludes_replaces_default() {
        let toml = "[language.python]\nexcludes = [\"worked/**\"]\n";
        let c = Config::from_toml(toml).unwrap();
        assert_eq!(c.language.python.excludes, vec!["worked/**".to_string()]);
        // The default is NOT silently merged in.
        assert!(!c
            .language
            .python
            .excludes
            .contains(&"__pycache__/**".to_string()));
    }

    #[test]
    fn explicit_empty_excludes_opts_out() {
        let toml = "[language.python]\nexcludes = []\n";
        let c = Config::from_toml(toml).unwrap();
        assert!(c.language.python.excludes.is_empty());
    }

    #[test]
    fn parses_targets() {
        let toml = r#"
[language.python]
enabled = true
targets = ["src/api", "src/worker"]
"#;
        let c = Config::from_toml(toml).unwrap();
        assert_eq!(
            c.language.python.targets,
            vec!["src/api".to_string(), "src/worker".to_string()]
        );
    }

    /// Legacy `exclude_documents` field is rejected by `deny_unknown_fields`.
    /// Migration path: rename to `excludes`.
    #[test]
    fn legacy_exclude_documents_field_errors() {
        let toml = "[language.python]\nexclude_documents = [\"foo/**\"]\n";
        let err = Config::from_toml(toml).unwrap_err();
        match err {
            ConfigError::Toml(_) => {}
            other => panic!("expected Toml deny_unknown_fields error, got {other:?}"),
        }
    }
}
