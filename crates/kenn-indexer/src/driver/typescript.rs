use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::canonicalize::Workspace;
use crate::docker::ContainerMount;
use crate::report::{RunReport, RunStatus};

use super::{container_arg, spawn_stderr_capture, DriverError, JsonlIndexer, JsonlOutcome};

/// kenn-ts driver: streams JSONL frames on the subprocess's stdout for the
/// entire workspace in one invocation. The indexer discovers `tsconfig.json`
/// projects itself (honoring git-worktree exclusion); a non-empty `projects`
/// list overrides discovery and is forwarded as `--tsconfigs`.
pub struct KennTs {
    /// Launcher tokens — `command[0]` is the program (subject to the
    /// Tier-2 PATH probe) and `command[1..]` are leading arguments
    /// prepended to the driver's intrinsic arg list. Default
    /// `["kenn-ts"]`.
    pub command: Vec<String>,
    /// Workspace-relative tsconfig dirs/paths to index. Empty = discover.
    pub projects: Vec<PathBuf>,
    /// Host→container path translation for the Windows docker `Translate` mount.
    /// `None` (local, or POSIX same-path) passes host paths through.
    pub mount: Option<ContainerMount>,
}

impl Default for KennTs {
    fn default() -> Self {
        Self {
            command: vec!["kenn-ts".into()],
            projects: Vec::new(),
            mount: None,
        }
    }
}

impl JsonlIndexer for KennTs {
    fn language_id(&self) -> &'static str {
        "typescript"
    }

    fn command(&self) -> PathBuf {
        PathBuf::from(self.command.first().map_or("kenn-ts", String::as_str))
    }

    fn run(&self, workspace: &Workspace) -> Result<JsonlOutcome, DriverError> {
        let program = self.command.first().map_or("kenn-ts", String::as_str);
        let mut report = RunReport::started_for(
            kenn_model::Language::TypeScript,
            "kenn-ts",
            "?",
            &format!("kenn-ts@{}", workspace.root().display()),
        );

        let mut cmd = Command::new(program);
        cmd.args(self.command.iter().skip(1));
        cmd.arg("index")
            .arg("--workspace")
            .arg(container_arg(self.mount.as_ref(), workspace.root()));
        // `--tsconfigs` are workspace-relative; the indexer resolves them against
        // the workspace, so they need no host→container translation.
        for p in &self.projects {
            cmd.arg("--tsconfigs").arg(p);
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
                    .push("kenn-ts not found on PATH".into());
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
