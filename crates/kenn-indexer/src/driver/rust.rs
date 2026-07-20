use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::canonicalize::Workspace;
use crate::report::{RunReport, RunStatus};

use super::{
    error_reason, make_scip_output_path, walk_for_language, DriverError, ScipDriver, ScipOutcome,
    Unit,
};

/// rust-analyzer SCIP driver. Runs `rust-analyzer scip <dir> --output …`
/// once per Cargo *root* — every `[workspace]` manifest plus any standalone
/// `[package]` crate not already under one. Per the spike on real repos
/// (rust-analyzer itself, ~1.5k files, 261s warm), pointing at sub-crates does
/// NOT save time — RA loads the whole crate graph regardless — so a workspace's
/// member crates are folded into the single root run. Walking for every
/// `Cargo.toml` (rather than only `<ws-root>/Cargo.toml`) means a crate nested
/// in a polyglot repo is indexed instead of silently skipped.
pub struct RustAnalyzer {
    /// Launcher tokens — `command[0]` is the program (subject to the
    /// Tier-2 PATH probe) and `command[1..]` are leading arguments
    /// prepended to the driver's intrinsic arg list. Default
    /// `["rust-analyzer"]`.
    pub command: Vec<String>,
    /// Pass `--exclude-vendored-libraries` (skip code from `vendor/`).
    pub exclude_vendored_libraries: bool,
    /// Cap rayon parallelism via `RAYON_NUM_THREADS`. `None` lets
    /// rust-analyzer use its own default (physical core count).
    pub max_threads: Option<usize>,
    /// Lower scheduler priority via `setpriority` for the subprocess
    /// (Unix). Equivalent to `nice -n 10`. Windows: no-op.
    pub low_priority: bool,
}

impl Default for RustAnalyzer {
    fn default() -> Self {
        Self {
            command: vec!["rust-analyzer".into()],
            exclude_vendored_libraries: true,
            max_threads: None,
            low_priority: false,
        }
    }
}

impl ScipDriver for RustAnalyzer {
    fn language_id(&self) -> &'static str {
        "rust"
    }

    fn command(&self) -> PathBuf {
        PathBuf::from(self.command.first().map_or("rust-analyzer", String::as_str))
    }

    fn discover_units(&self, workspace: &Workspace) -> Result<Vec<Unit>, DriverError> {
        // Classify every `Cargo.toml` the walk surfaces (excludes prune
        // `target/`, vendored dirs) as a workspace root or a package.
        let mut roots: Vec<PathBuf> = Vec::new();
        let mut packages: Vec<PathBuf> = Vec::new();
        for entry in walk_for_language(workspace, kenn_model::Language::Rust) {
            let entry = entry?;
            if entry.file_name().and_then(|s| s.to_str()) != Some("Cargo.toml") {
                continue;
            }
            let Some(dir) = entry.parent().map(Path::to_path_buf) else {
                continue;
            };
            match classify_cargo_manifest(&entry) {
                CargoRole::Workspace => roots.push(dir),
                CargoRole::Package => packages.push(dir),
                CargoRole::Neither => {}
            }
        }
        // A `[package]` under a discovered workspace root is a member — the
        // root's single RA run already covers it (RA loads the whole graph),
        // so drop it to avoid a redundant, overlapping invocation.
        let mut cargo_dirs: Vec<PathBuf> = roots.clone();
        for pkg in packages {
            if !roots.iter().any(|r| pkg.starts_with(r)) {
                cargo_dirs.push(pkg);
            }
        }
        cargo_dirs.sort();
        cargo_dirs.dedup();

        let root = workspace.root();
        Ok(cargo_dirs
            .into_iter()
            .map(|dir| {
                // The workspace-root crate keeps the bare `rust` slug so its
                // scip filename is stable; a nested root gets a path-derived
                // slug so multiple outputs never collide.
                let identifier = if dir == root {
                    "rust".to_string()
                } else {
                    let rel = dir.strip_prefix(root).unwrap_or(&dir).to_string_lossy();
                    format!("rust-{}", rel.replace(['/', '\\'], "-"))
                };
                Unit {
                    identifier,
                    path: dir,
                }
            })
            .collect())
    }

    fn run_unit(&self, unit: &Unit, workspace: &Workspace) -> Result<ScipOutcome, DriverError> {
        let program = self.command.first().map_or("rust-analyzer", String::as_str);
        let output = make_scip_output_path(workspace, &unit.identifier)?;
        let mut report = RunReport::started_for(
            kenn_model::Language::Rust,
            "rust-analyzer",
            "?",
            &unit.identifier,
        );

        let mut cmd = Command::new(program);
        cmd.args(self.command.iter().skip(1));
        cmd.arg("scip").arg(&unit.path).arg("--output").arg(&output);
        if self.exclude_vendored_libraries {
            cmd.arg("--exclude-vendored-libraries");
        }
        // Isolate `cargo metadata`'s target-dir lock from the user's
        // workspace `target/` — rust-analyzer scip runs cargo metadata
        // internally, which acquires the same file lock as `cargo
        // build` / `cargo clippy`. Without isolation, an indexer run
        // and a developer build block each other for minutes.
        //
        // Per-workspace path (`<workspace-root>/.kenn/local/cargo-target/`)
        // also isolates concurrent agents that work in separate git
        // worktrees: each worktree's `workspace.root()` is a distinct
        // filesystem path, so their RA target dirs never collide. We
        // don't need RA's artifacts long-term, only its metadata output
        // for SCIP, so a sibling dir under `.kenn/` is the right home.
        cmd.env(
            "CARGO_TARGET_DIR",
            workspace.root().join(".kenn/local/cargo-target"),
        );
        // Cap rayon parallelism inside rust-analyzer scip. The scip
        // subcommand has no CLI flag for this (analysis-stats has
        // `--num-threads`, scip doesn't), so `RAYON_NUM_THREADS` is the
        // only knob. Default (None) leaves rust-analyzer's own default
        // (physical core count) intact.
        if let Some(n) = self.max_threads {
            cmd.env("RAYON_NUM_THREADS", n.to_string());
        }
        // Lower scheduler priority on Unix when the operator opted in.
        // The kernel still gives the subprocess CPU when nothing else
        // wants it, but yields to foreground work. On macOS the nice
        // value also nudges the scheduler toward E-cores, reducing fan
        // noise. Windows: skipped (kenn isn't Windows-targeted today).
        #[cfg(unix)]
        if self.low_priority {
            lower_subprocess_priority(&mut cmd);
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let out = match cmd.output() {
            Ok(o) => o,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.status = RunStatus::Failed;
                report
                    .failed_projects
                    .push("rust-analyzer not found on PATH".into());
                report.finalize();
                return Ok(ScipOutcome::Unavailable { report });
            }
            Err(e) => return Err(e.into()),
        };
        if !out.status.success() {
            report.status = RunStatus::Failed;
            let stderr = String::from_utf8_lossy(&out.stderr);
            report.failed_projects.push(format!(
                "rust-analyzer exited {:?}: {}",
                out.status.code(),
                error_reason(&stderr)
            ));
            report.finalize();
            return Ok(ScipOutcome::Unavailable { report });
        }
        Ok(ScipOutcome::Scip {
            path: output,
            report,
        })
    }
}

/// A `Cargo.toml`'s role in cargo-root discovery.
enum CargoRole {
    /// Declares `[workspace]` — a root RA indexes as a whole.
    Workspace,
    /// Declares `[package]` (and no `[workspace]`) — a standalone crate,
    /// unless it turns out to be a member under a discovered workspace root.
    Package,
    /// Unreadable, malformed, or declares neither table.
    Neither,
}

#[derive(Deserialize)]
struct CargoManifest {
    package: Option<toml::Value>,
    workspace: Option<toml::Value>,
}

/// Classify a `Cargo.toml` by its top-level tables. A manifest with both
/// `[workspace]` and `[package]` is a root (RA loads the whole graph from it),
/// so `[workspace]` wins.
fn classify_cargo_manifest(manifest: &Path) -> CargoRole {
    let Ok(text) = std::fs::read_to_string(manifest) else {
        return CargoRole::Neither;
    };
    match toml::from_str::<CargoManifest>(&text) {
        Ok(m) if m.workspace.is_some() => CargoRole::Workspace,
        Ok(m) if m.package.is_some() => CargoRole::Package,
        _ => CargoRole::Neither,
    }
}

/// Hook `setpriority(PRIO_PROCESS, 0, 10)` into the child via
/// `Command::pre_exec` — equivalent to `nice -n 10`. Runs in the
/// child between fork and exec, restricted to async-signal-safe ops
/// (which `setpriority` is, per POSIX). The result is intentionally
/// ignored — failure leaves the subprocess at normal priority, which
/// is still useful.
#[cfg(unix)]
fn lower_subprocess_priority(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    #[expect(unsafe_code, reason = "Command::pre_exec is unsafe by signature.")]
    // SAFETY: `pre_exec` requires its closure body to be async-signal-
    // safe in the post-fork pre-exec window. `child_nice_self` makes a
    // single FFI call to POSIX-async-signal-safe `setpriority` — no
    // allocation, no locking, no Rust mutable statics — and returns.
    unsafe {
        cmd.pre_exec(child_nice_self);
    }
}

/// Post-fork pre-exec hook: `nice +10` the calling process. Body is
/// async-signal-safe so it's legal in the post-fork window. The
/// `io::Result<()>` return type is required by [`std::os::unix::process::CommandExt::pre_exec`];
/// we never produce an `Err` (setpriority failure is best-effort).
#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "pre_exec's callback signature requires io::Result<()>."
)]
fn child_nice_self() -> std::io::Result<()> {
    #[expect(unsafe_code, reason = "FFI to async-signal-safe setpriority(2).")]
    // SAFETY: FFI to `setpriority(PRIO_PROCESS, 0, 10)` — POSIX
    // async-signal-safe with integer-only arguments. Return value
    // intentionally dropped: if setpriority fails (e.g. RLIMIT_NICE
    // denies the priority increase), the subprocess just runs at
    // normal priority — still useful.
    let _ = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 10) };
    Ok(())
}
