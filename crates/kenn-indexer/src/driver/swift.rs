use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::canonicalize::Workspace;
use crate::docker::ContainerMount;
use crate::report::{RunReport, RunStatus};

use super::{container_arg, spawn_stderr_capture, DriverError, JsonlIndexer, JsonlOutcome};

/// kenn-swift driver: streams JSONL frames on the subprocess's stdout for the
/// entire workspace in one invocation.
///
/// Spawns `kenn-swift index --workspace <ws> [--projects <a> <b>] [--skip-build]`.
///
/// DISCOVERY LIVES IN THE SIDECAR, not here — unlike the other drivers
/// (`KennDotnet`/`RustAnalyzer`/`ScipPython` walk for `.sln`/`Cargo.toml`/`.py`
/// in Rust). This is deliberate: Swift's Xcode projects are *directory bundles*
/// (`.xcodeproj`/`.xcworkspace`), which the file-only `walk_for_language` cannot
/// see as units — it yields the files *inside* the bundle. So the sidecar
/// (`Provisioning.discoverProjects`, using `FileManager` bundle-aware traversal)
/// owns discovery of BOTH `SwiftPM` packages and Xcode projects. Do NOT "restore
/// symmetry" by moving discovery back into Rust — it re-breaks Xcode bundles.
/// This driver only passes explicit `projects` through (or nothing, letting the
/// sidecar discover).
pub struct KennSwift {
    /// Launcher tokens — `command[0]` is the program (subject to the Tier-2
    /// PATH probe) and `command[1..]` are leading arguments prepended to the
    /// driver's intrinsic arg list. Default `["kenn-swift"]`.
    pub command: Vec<String>,
    /// Skip the build pass that produces the index store (the sidecar reads an
    /// already-built store only — `.build/.../index/store` for `SwiftPM`, the
    /// Xcode derived-data `Index.noindex/DataStore`).
    pub skip_build: bool,
    /// Workspace-relative project paths to index — `Package.swift`,
    /// `.xcodeproj`, or `.xcworkspace` (the sidecar classifies by extension).
    /// When empty, the sidecar discovers both kinds itself (see the type-level
    /// note on why discovery is not done here).
    pub projects: Vec<PathBuf>,
    /// Xcode build-destination override (`ios`/`macos`/…). `None` = sidecar
    /// auto-detects. Forwarded as `--platform`; ignored for `SwiftPM` packages.
    pub platform: Option<String>,
    /// Host→container path translation for the Windows docker `Translate` mount.
    /// `None` (local, or POSIX same-path) passes host paths through.
    pub mount: Option<ContainerMount>,
}

impl Default for KennSwift {
    fn default() -> Self {
        Self {
            command: vec!["kenn-swift".into()],
            skip_build: false,
            projects: Vec::new(),
            platform: None,
            mount: None,
        }
    }
}

impl KennSwift {
    /// Resolve the package list passed to one kenn-swift invocation, as
    /// absolute paths under the workspace.
    ///
    /// - If `self.projects` is non-empty, every path is resolved against the
    ///   workspace root and verified to exist; a missing path is an error so a
    ///   stale `kenn.toml` is noticed. Entries may be `Package.swift`,
    ///   `.xcodeproj`, or `.xcworkspace` — the sidecar classifies by extension.
    /// - Otherwise, returns empty and the sidecar discovers both `SwiftPM`
    ///   packages and Xcode projects itself (it can see `.xcodeproj`/
    ///   `.xcworkspace` bundle dirs, which a file-walk here cannot).
    pub(crate) fn resolve_projects(
        &self,
        workspace: &Workspace,
    ) -> Result<Vec<PathBuf>, DriverError> {
        let mut out = Vec::with_capacity(self.projects.len());
        for rel in &self.projects {
            let abs = workspace.root().join(rel);
            if !abs.exists() {
                return Err(DriverError::Subprocess(format!(
                    "configured project not found: {}",
                    rel.display()
                )));
            }
            out.push(abs);
        }
        Ok(out)
    }

    /// Synthetic identifier for the single `RunReport` covering this
    /// invocation (all configured packages share one report).
    fn run_identifier(workspace: &Workspace) -> String {
        format!("kenn-swift@{}", workspace.root().display())
    }
}

impl JsonlIndexer for KennSwift {
    fn language_id(&self) -> &'static str {
        "swift"
    }

    fn command(&self) -> PathBuf {
        PathBuf::from(self.command.first().map_or("kenn-swift", String::as_str))
    }

    fn run(&self, workspace: &Workspace) -> Result<JsonlOutcome, DriverError> {
        let program = self.command.first().map_or("kenn-swift", String::as_str);

        let projects = self.resolve_projects(workspace)?;
        let mut report = RunReport::started_for(
            kenn_model::Language::Swift,
            "kenn-swift",
            "?",
            &Self::run_identifier(workspace),
        );

        let mut cmd = Command::new(program);
        cmd.args(self.command.iter().skip(1));
        cmd.arg("index")
            .arg("--workspace")
            .arg(container_arg(self.mount.as_ref(), workspace.root()));
        for pkg in &projects {
            cmd.arg("--projects")
                .arg(container_arg(self.mount.as_ref(), pkg));
        }
        if self.skip_build {
            cmd.arg("--skip-build");
        }
        if let Some(platform) = &self.platform {
            cmd.arg("--platform").arg(platform);
        }
        let stream_path = workspace
            .jsonl_stream_path(self.language_id())
            .map_err(|e| DriverError::Subprocess(format!("allocate JSONL stream path: {e}")))?;
        let stream_file = std::fs::File::create(&stream_path).map_err(|e| {
            DriverError::Subprocess(format!("create stream file {}: {e}", stream_path.display()))
        })?;
        cmd.stdout(Stdio::from(stream_file)).stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.status = RunStatus::Failed;
                report
                    .failed_projects
                    .push("kenn-swift not found on PATH".into());
                drop(std::fs::remove_file(&stream_path));
                return Ok(JsonlOutcome::Unavailable { report });
            }
            Err(e) => {
                drop(std::fs::remove_file(&stream_path));
                return Err(e.into());
            }
        };
        let stderr = child.stderr.take().map(spawn_stderr_capture);
        Ok(JsonlOutcome::Jsonl {
            child,
            stream_path,
            stderr,
            report,
        })
    }
}
