//! `[language.typescript]` — kenn-ts JSONL indexer config.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::Runtime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypescriptConfig {
    /// Disabled by default — opt in via `[language.typescript] enabled = true`.
    #[serde(default)]
    pub enabled: bool,
    /// Launcher tokens. Default `["kenn-ts"]` — PATH lookup.
    #[serde(default = "default_typescript_command")]
    pub command: Vec<String>,
    /// Indexer runtime: `"local"` (default, host `PATH`) or `"docker"` (run
    /// `command` inside `image`). See the `docker-indexer-runtime` change.
    #[serde(default)]
    pub runtime: Runtime,
    /// OCI image used when `runtime = "docker"` — required then, ignored
    /// otherwise (validated in `Config::validate`).
    #[serde(default)]
    pub image: Option<String>,
    /// Workspace-relative tsconfig directories (or `tsconfig.json` paths)
    /// to index, overriding auto-discovery. Empty = walk the workspace.
    #[serde(default)]
    pub projects: Vec<PathBuf>,
    /// Workspace-relative glob patterns excluded from TypeScript
    /// discovery AND ingest. Scoped to the TS pipeline only.
    /// User-supplied values REPLACE the default fully.
    #[serde(default = "default_typescript_excludes")]
    pub excludes: Vec<String>,
}

impl TypescriptConfig {
    /// Workspace-walk exclude defaults specific to TypeScript / Node.
    /// `bin/` is intentionally omitted — TS projects routinely ship
    /// `bin/` scripts as source.
    pub const DEFAULT_EXCLUDES: &'static [&'static str] =
        &["node_modules/**", "**/node_modules/**"];
}

fn default_typescript_command() -> Vec<String> {
    vec!["kenn-ts".into()]
}

fn default_typescript_excludes() -> Vec<String> {
    TypescriptConfig::DEFAULT_EXCLUDES
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

impl Default for TypescriptConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            command: default_typescript_command(),
            runtime: Runtime::Local,
            image: None,
            projects: Vec::new(),
            excludes: default_typescript_excludes(),
        }
    }
}
