//! `[language.swift]` — kenn-swift JSONL indexer config.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::Runtime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwiftConfig {
    /// Disabled by default — opt in via `[language.swift] enabled = true`.
    #[serde(default)]
    pub enabled: bool,
    /// Launcher tokens. Default `["kenn-swift"]` — PATH lookup.
    #[serde(default = "default_swift_command")]
    pub command: Vec<String>,
    /// Indexer runtime: `"local"` (default, host `PATH`) or `"docker"` (run
    /// `command` inside `image`). See the `docker-indexer-runtime` change.
    #[serde(default)]
    pub runtime: Runtime,
    /// OCI image used when `runtime = "docker"` — required then, ignored
    /// otherwise (validated in `Config::validate`).
    #[serde(default)]
    pub image: Option<String>,
    /// Workspace-relative `Package.swift` paths to index, overriding
    /// auto-discovery. Empty (default) = walk the workspace and pick up
    /// every `SwiftPM` package.
    #[serde(default)]
    pub projects: Vec<PathBuf>,
    /// Skip the `swift build` pass that produces the index store, reading an
    /// already-built `.build/index/store` only. Set when a prior build is
    /// guaranteed (CI) or when offline; the sidecar errors if no store exists.
    #[serde(default)]
    pub skip_build: bool,
    /// Xcode build-destination override for multiplatform apps (`ios`, `macos`,
    /// `tvos`, `watchos`, `visionos`). `None` (default) lets the sidecar
    /// auto-detect from the scheme's `SUPPORTED_PLATFORMS` (preferring macOS).
    /// Ignored for `SwiftPM` packages.
    #[serde(default)]
    pub platform: Option<String>,
    /// Workspace-relative glob patterns excluded from Swift discovery AND
    /// ingest. User-supplied values REPLACE the default fully.
    #[serde(default = "default_swift_excludes")]
    pub excludes: Vec<String>,
}

impl SwiftConfig {
    /// Workspace-walk exclude defaults specific to `SwiftPM` (build output).
    pub const DEFAULT_EXCLUDES: &'static [&'static str] = &[".build/**", "**/.build/**"];
}

fn default_swift_command() -> Vec<String> {
    vec!["kenn-swift".into()]
}

fn default_swift_excludes() -> Vec<String> {
    SwiftConfig::DEFAULT_EXCLUDES
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

impl Default for SwiftConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            command: default_swift_command(),
            runtime: Runtime::Local,
            image: None,
            projects: Vec::new(),
            skip_build: false,
            platform: None,
            excludes: default_swift_excludes(),
        }
    }
}
