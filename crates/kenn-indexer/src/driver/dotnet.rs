use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::canonicalize::Workspace;
use crate::report::{RunReport, RunStatus};

use kenn_model::Language;

use super::{spawn_stderr_capture, walk_for_language, DriverError, JsonlIndexer, JsonlOutcome};

/// kenn-dotnet driver: streams JSONL frames on the subprocess's stdout for
/// the entire workspace in one invocation.
///
/// Spawns `kenn-dotnet index --workspace <ws> --projects <a> <b> <c>
/// [--skip-restore]`. The list of `.sln`/`.csproj` paths comes from
/// `kenn.toml`'s `[language.csharp].projects`; if empty, falls back to
/// walk-based discovery (prefer `.sln`, fall back to `.csproj`).
pub struct KennDotnet {
    /// Launcher tokens — `command[0]` is the program (subject to the
    /// Tier-2 PATH probe) and `command[1..]` are leading arguments
    /// prepended to the driver's intrinsic arg list. Default
    /// `["kenn-dotnet"]`.
    pub command: Vec<String>,
    /// Skip the `dotnet restore` pass (caller is responsible for restoring).
    /// Default **false** — restore runs. Skipping it silently unbinds every
    /// `NuGet` type (they degrade to bare syntactic names with no diagnostic),
    /// which only looks harmless on a dev machine whose `obj/` is already
    /// restored. Set from `[language.csharp] restore`.
    pub skip_restore: bool,
    /// Workspace-relative `.sln`/`.csproj` paths to index. When empty,
    /// `run` discovers units by walking the workspace.
    pub projects: Vec<PathBuf>,
    /// Glob patterns forwarded as `--test-glob`. The C# side tags symbols
    /// in matching files with `test = true`. Empty = no files tagged.
    pub test_globs: Vec<String>,
    /// Regexes forwarded as `--test-assembly-regex`. The C# side marks a
    /// project whose assembly name matches any as test code. Empty = none.
    pub test_assembly_regexes: Vec<String>,
}

impl Default for KennDotnet {
    fn default() -> Self {
        Self {
            command: vec!["kenn-dotnet".into()],
            skip_restore: false,
            projects: Vec::new(),
            test_globs: Vec::new(),
            test_assembly_regexes: Vec::new(),
        }
    }
}

impl KennDotnet {
    /// Resolve the project list passed to one kenn-dotnet invocation.
    /// Returns absolute paths in workspace.
    ///
    /// - If `self.projects` is non-empty, every path is resolved against
    ///   the workspace root and verified to exist; missing paths cause a
    ///   `Subprocess` error so the user notices a stale `kenn.toml`.
    /// - Otherwise, walks the workspace and prefers `.sln` files; falls
    ///   back to `.csproj` only if no `.sln` is present.
    pub(crate) fn resolve_projects(
        &self,
        workspace: &Workspace,
    ) -> Result<Vec<PathBuf>, DriverError> {
        if !self.projects.is_empty() {
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
            return Ok(out);
        }
        // Prefer a solution over loose `.csproj`s: a solution names the exact
        // project set and lets the sidecar restore it in one call. `.slnx` is
        // the newer XML solution format (MSBuild 17.13+) — matched alongside
        // `.sln` because a repo that ships only it (Newtonsoft.Json) otherwise
        // falls through to per-`.csproj`, and a bare `dotnet restore` on a
        // nested `.csproj` fails from the workspace root.
        let mut slns = Vec::new();
        let mut csprojs = Vec::new();
        for entry in walk_for_language(workspace, Language::Csharp) {
            let entry = entry?;
            match entry.extension().and_then(|s| s.to_str()) {
                Some("sln" | "slnx") => slns.push(entry),
                Some("csproj") => csprojs.push(entry),
                _ => {}
            }
        }
        Ok(if slns.is_empty() { csprojs } else { slns })
    }

    /// Synthetic identifier for the single `RunReport` covering this
    /// invocation. The report covers all configured projects, so the
    /// identifier is workspace-scoped rather than per-`.sln`.
    fn run_identifier(workspace: &Workspace) -> String {
        format!("kenn-dotnet@{}", workspace.root().display())
    }
}

impl JsonlIndexer for KennDotnet {
    fn language_id(&self) -> &'static str {
        "csharp"
    }

    fn command(&self) -> PathBuf {
        PathBuf::from(self.command.first().map_or("kenn-dotnet", String::as_str))
    }

    fn run(&self, workspace: &Workspace) -> Result<JsonlOutcome, DriverError> {
        let program = self.command.first().map_or("kenn-dotnet", String::as_str);

        let projects = self.resolve_projects(workspace)?;
        let mut report = RunReport::started_for(
            kenn_model::Language::Csharp,
            "kenn-dotnet",
            "?",
            &Self::run_identifier(workspace),
        );

        let mut cmd = Command::new(program);
        cmd.args(self.command.iter().skip(1));
        cmd.arg("index").arg("--workspace").arg(workspace.root());
        for sln in &projects {
            cmd.arg("--projects").arg(sln);
        }
        for pat in &self.test_globs {
            cmd.arg("--test-glob").arg(pat);
        }
        for pat in &self.test_assembly_regexes {
            cmd.arg("--test-assembly-regex").arg(pat);
        }
        if self.skip_restore {
            cmd.arg("--skip-restore");
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
                    .push("kenn-dotnet not found on PATH".into());
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
