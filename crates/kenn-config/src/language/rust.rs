//! `[language.rust]` — rust-analyzer SCIP indexer config.

use serde::{Deserialize, Serialize};

use super::Runtime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustConfig {
    /// Disabled by default — opt in via `[language.rust] enabled = true`.
    /// Many repos have a stray `Cargo.toml` (vendored deps, examples) and
    /// rust-analyzer is heavy enough that we don't want to surprise users.
    #[serde(default)]
    pub enabled: bool,
    /// Launcher tokens. Default `["rust-analyzer"]` — PATH lookup. Set to
    /// e.g. `["/opt/rust/bin/rust-analyzer"]` for an absolute path, or
    /// `["asdf", "exec", "rust-analyzer"]` to route through a wrapper.
    #[serde(default = "default_rust_command")]
    pub command: Vec<String>,
    /// Indexer runtime: `"local"` (default, host `PATH`) or `"docker"` (run
    /// `command` inside `image`). See the `docker-indexer-runtime` change.
    #[serde(default)]
    pub runtime: Runtime,
    /// OCI image used when `runtime = "docker"` — required then, ignored
    /// otherwise (validated in `Config::validate`).
    #[serde(default)]
    pub image: Option<String>,
    /// Pass `--exclude-vendored-libraries`. Default true.
    #[serde(default = "default_true")]
    pub exclude_vendored_libraries: bool,
    /// Cap the number of CPU threads `rust-analyzer scip` uses (via
    /// `RAYON_NUM_THREADS`). `None` (default) keeps rust-analyzer's
    /// own default (physical core count). Set to a small value (e.g.
    /// `4`) to reduce peak CPU spikes during indexing — useful on
    /// laptops where the cooler is noisy under full-fan workloads.
    /// The scip subcommand has no equivalent CLI flag; this is the
    /// only knob.
    #[serde(default)]
    pub max_threads: Option<usize>,
    /// Run the `rust-analyzer scip` subprocess at a lowered scheduler
    /// priority via `setpriority(PRIO_PROCESS, 0, 10)` (equivalent to
    /// `nice -n 10`). On macOS the nice value also nudges the
    /// scheduler toward E-cores, reducing fan noise. Foreground work
    /// preempts the indexer; indexing wall-clock grows slightly when
    /// the system is busy. Windows: no effect today. Default `false`
    /// so first-time users see indexing complete as fast as possible.
    #[serde(default)]
    pub low_priority: bool,
    /// Workspace-relative glob patterns excluded from Rust discovery
    /// AND ingest. Scoped to the Rust pipeline only — does not affect
    /// other languages. User-supplied values REPLACE the default fully
    /// (`excludes = []` opts out completely).
    #[serde(default = "default_rust_excludes")]
    pub excludes: Vec<String>,
}

impl RustConfig {
    /// Workspace-walk exclude defaults specific to Rust. Used as the
    /// serde default for `excludes` and as the documented constant.
    /// NOTE: `src/bin/` is a Rust binary-source convention — `bin/**`
    /// is intentionally NOT included.
    pub const DEFAULT_EXCLUDES: &'static [&'static str] = &["target/**", "**/target/**"];
}

fn default_rust_command() -> Vec<String> {
    vec!["rust-analyzer".into()]
}

fn default_rust_excludes() -> Vec<String> {
    RustConfig::DEFAULT_EXCLUDES
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

const fn default_true() -> bool {
    true
}

impl Default for RustConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            command: default_rust_command(),
            runtime: Runtime::Local,
            image: None,
            exclude_vendored_libraries: true,
            max_threads: None,
            low_priority: false,
            excludes: default_rust_excludes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RustConfig;
    use crate::language::Runtime;

    #[test]
    fn runtime_and_image_default_to_local_none_and_round_trip() {
        // Absent from the config → the local default.
        let d = RustConfig::default();
        assert_eq!(d.runtime, Runtime::Local);
        assert_eq!(d.image, None);
        let bare: RustConfig = toml::from_str("enabled = true").unwrap();
        assert_eq!(bare.runtime, Runtime::Local);
        assert_eq!(bare.image, None);

        // Present → parsed (`runtime` from the lowercase enum rename).
        let c: RustConfig =
            toml::from_str("runtime = \"docker\"\nimage = \"ghcr.io/kenn/ra@sha256:a\"").unwrap();
        assert_eq!(c.runtime, Runtime::Docker);
        assert_eq!(c.image.as_deref(), Some("ghcr.io/kenn/ra@sha256:a"));

        // deny_unknown_fields still holds with the new fields present.
        toml::from_str::<RustConfig>("bogus = true").unwrap_err();
    }
}
