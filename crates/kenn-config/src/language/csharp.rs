//! `[language.csharp]` — kenn-dotnet JSONL indexer config.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::Runtime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
// A config struct of independent user toggles — each maps to one kenn.toml key
// and is set in isolation. Bundling them into an enum would obscure the 1:1
// mapping the whole file exists to express.
#[expect(clippy::struct_excessive_bools, reason = "independent config toggles")]
pub struct CsharpConfig {
    /// Disabled by default — opt in via `[language.csharp] enabled = true`.
    /// Aligned with the other languages so workspaces without C# don't
    /// invoke `kenn-dotnet` and fail with "projects=0".
    #[serde(default)]
    pub enabled: bool,
    /// Launcher tokens. Default `["kenn-dotnet"]` — PATH lookup.
    #[serde(default = "default_csharp_command")]
    pub command: Vec<String>,
    /// Indexer runtime: `"local"` (default, host `PATH`) or `"docker"` (run
    /// `command` inside `image`). See the `docker-indexer-runtime` change.
    #[serde(default)]
    pub runtime: Runtime,
    /// OCI image used when `runtime = "docker"` — required then, ignored
    /// otherwise (validated in `Config::validate`).
    #[serde(default)]
    pub image: Option<String>,
    /// Workspace-relative `.sln` / `.csproj` paths to index, overriding
    /// auto-discovery. Empty (default) = walk the workspace and pick up
    /// every solution. Useful when a repo vendors third-party libraries
    /// that ship their own `.sln` files (those crash `MSBuild` on macOS when
    /// targeting frameworks the local SDK can't satisfy, and their content
    /// usually overlaps the main solution anyway).
    #[serde(default)]
    pub projects: Vec<PathBuf>,
    /// Run `dotnet restore` before indexing. Default true.
    ///
    /// Without a restore, `NuGet` assemblies never bind: every package type
    /// degrades to a bare syntactic name (`JsonSerializerSettings` instead of
    /// `Newtonsoft.Json.JsonSerializerSettings`) and the run still exits 0 with
    /// symbols and no diagnostic — a silent fidelity loss. A dev machine
    /// usually has a restored `obj/` from its own builds, which is why this
    /// went unnoticed; a fresh container (`runtime = "docker"`) or a clean CI
    /// checkout has none. Set false only when the caller guarantees the
    /// workspace is already restored, to skip a redundant restore pass.
    #[serde(default = "default_true")]
    pub restore: bool,
    /// When true, the CLI MAY (with explicit user opt-in) provision a
    /// `Directory.Build.props` for the workspace. Default false: we never
    /// modify a user's repo without consent.
    #[serde(default)]
    pub provision_directory_build_props: bool,
    /// When true, install a project's pinned SDK on demand if it is missing,
    /// then retry — the nested-`global.json` case, where a subdirectory pins an
    /// SDK the entrypoint did not provision. Default false: it reaches the
    /// network at index time, and off keeps an unsatisfiable pin the named,
    /// terminal failure it is today.
    #[serde(default)]
    pub provision_sdk: bool,
    /// Workspace-relative glob patterns excluded from C# discovery AND
    /// ingest. Scoped to the C# pipeline only. User-supplied values
    /// REPLACE the default fully.
    #[serde(default = "default_csharp_excludes")]
    pub excludes: Vec<String>,
}

impl CsharpConfig {
    /// Workspace-walk exclude defaults specific to .NET (`MSBuild` output).
    pub const DEFAULT_EXCLUDES: &'static [&'static str] =
        &["bin/**", "**/bin/**", "obj/**", "**/obj/**"];
}

fn default_csharp_command() -> Vec<String> {
    vec!["kenn-dotnet".into()]
}

const fn default_true() -> bool {
    true
}

fn default_csharp_excludes() -> Vec<String> {
    CsharpConfig::DEFAULT_EXCLUDES
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

impl Default for CsharpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            command: default_csharp_command(),
            runtime: Runtime::Local,
            image: None,
            projects: Vec::new(),
            restore: true,
            provision_directory_build_props: false,
            provision_sdk: false,
            excludes: default_csharp_excludes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CsharpConfig;

    #[test]
    fn restore_defaults_true_and_round_trips() {
        // Restoring by default is the whole fix: an unrestored project binds no
        // NuGet type and says nothing about it. Absent from the config → true.
        assert!(CsharpConfig::default().restore);
        let bare: CsharpConfig = toml::from_str("enabled = true").unwrap();
        assert!(bare.restore, "absent `restore` must default to restoring");

        // Explicit opt-out for callers that pre-restore.
        let off: CsharpConfig = toml::from_str("restore = false").unwrap();
        assert!(!off.restore);

        // deny_unknown_fields still holds with the new field present.
        toml::from_str::<CsharpConfig>("bogus = true").unwrap_err();
    }

    #[test]
    fn provision_sdk_defaults_off_and_opts_in() {
        assert!(!CsharpConfig::default().provision_sdk);
        let bare: CsharpConfig = toml::from_str("enabled = true").unwrap();
        assert!(!bare.provision_sdk, "absent = the strict default");
        let on: CsharpConfig = toml::from_str("provision_sdk = true").unwrap();
        assert!(on.provision_sdk);
    }
}
