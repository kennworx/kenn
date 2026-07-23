use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::canonicalize::Workspace;
use crate::docker::ContainerMount;
use crate::report::{RunReport, RunStatus};

use kenn_model::Language;

use super::{
    container_arg, error_reason, make_scip_output_path, walk_for_language, DriverError, ScipDriver,
    ScipOutcome, Unit,
};

/// scip-python SCIP driver. Spawns the configured launcher with `index
/// --cwd <ws> --output <out> --quiet [--target-only <dir>]`.
///
/// Discovery:
/// * Empty `targets` → at most one unit at the workspace root (only if
///   `.py`/`.pyi` is present after exclusions).
/// * Non-empty `targets` → one unit per entry, each verified to exist as
///   a directory at discovery time.
pub struct ScipPython {
    /// Launcher tokens — `command[0]` is the program (subject to the
    /// Tier-2 PATH probe), `command[1..]` are leading arguments
    /// prepended to the trailing `index ...` args. Defaults to
    /// `["scip-python"]`; users override with `["bunx",
    /// "@sourcegraph/scip-python"]`, `["npx", "--yes",
    /// "@sourcegraph/scip-python"]`, `["uvx", "scip-python"]`, etc.
    pub command: Vec<String>,
    /// Forwarded as `--project-name <name>` when set.
    pub project_name: Option<String>,
    /// Forwarded as `--project-version <ver>` when set.
    pub project_version: Option<String>,
    /// Workspace-relative sub-package directories. Empty = single
    /// whole-workspace invocation; non-empty = one scip-python
    /// invocation per entry, each with `--target-only <abs>`.
    pub targets: Vec<String>,
    /// Host→container path translation for the Windows docker `Translate` mount.
    /// `None` (local, or POSIX same-path) passes host paths through.
    pub mount: Option<ContainerMount>,
}

impl Default for ScipPython {
    fn default() -> Self {
        Self {
            command: vec!["scip-python".into()],
            project_name: None,
            project_version: None,
            targets: Vec::new(),
            mount: None,
        }
    }
}

impl ScipDriver for ScipPython {
    fn language_id(&self) -> &'static str {
        "python"
    }

    fn command(&self) -> PathBuf {
        PathBuf::from(self.command.first().map_or("scip-python", String::as_str))
    }

    fn container_mount(&self) -> Option<&ContainerMount> {
        self.mount.as_ref()
    }

    fn discover_units(&self, workspace: &Workspace) -> Result<Vec<Unit>, DriverError> {
        if !self.targets.is_empty() {
            let mut units = Vec::with_capacity(self.targets.len());
            for (idx, rel) in self.targets.iter().enumerate() {
                let abs = workspace.root().join(rel);
                if !abs.is_dir() {
                    return Err(DriverError::Subprocess(format!(
                        "configured python target not found: {rel}"
                    )));
                }
                units.push(Unit {
                    identifier: format!("python-{idx}"),
                    path: abs,
                });
            }
            return Ok(units);
        }
        // walk_for_language prunes `[workspace].excludes` AND
        // `[language.python].excludes` at directory recursion time,
        // so `.venv/`, `__pycache__/` etc. cost zero IO.
        for entry in walk_for_language(workspace, Language::Python) {
            let entry = entry?;
            if matches!(
                entry.extension().and_then(|s| s.to_str()),
                Some("py" | "pyi")
            ) {
                return Ok(vec![Unit {
                    identifier: "python-0".into(),
                    path: workspace.root().to_path_buf(),
                }]);
            }
        }
        Ok(Vec::new())
    }

    fn run_unit(&self, unit: &Unit, workspace: &Workspace) -> Result<ScipOutcome, DriverError> {
        let program = self.command.first().map_or("scip-python", String::as_str);
        let output = make_scip_output_path(workspace, &unit.identifier)?;
        let mut report = RunReport::started_for(
            kenn_model::Language::Python,
            "scip-python",
            "?",
            &unit.identifier,
        );

        let mut cmd = Command::new(program);
        cmd.args(self.command.iter().skip(1));
        // `output` stays the host path (read back after the run); only the args
        // the container sees are translated. The `unit.path != root` guard below
        // compares HOST paths (discovery is host-side) — correct as-is.
        cmd.arg("index")
            .arg("--cwd")
            .arg(container_arg(self.mount.as_ref(), workspace.root()))
            .arg("--output")
            .arg(container_arg(self.mount.as_ref(), &output))
            .arg("--quiet");
        if let Some(name) = self.project_name.as_deref() {
            cmd.arg("--project-name").arg(name);
        }
        if let Some(ver) = self.project_version.as_deref() {
            cmd.arg("--project-version").arg(ver);
        }
        if unit.path != workspace.root() {
            cmd.arg("--target-only")
                .arg(container_arg(self.mount.as_ref(), &unit.path));
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let out = match cmd.output() {
            Ok(o) => o,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.status = RunStatus::Failed;
                report.failed_projects.push(format!(
                    "scip-python launcher `{program}` not found on PATH"
                ));
                report.finalize();
                return Ok(ScipOutcome::Unavailable { report });
            }
            Err(e) => return Err(e.into()),
        };
        if !out.status.success() {
            report.status = RunStatus::Failed;
            let stderr = String::from_utf8_lossy(&out.stderr);
            report.failed_projects.push(format!(
                "scip-python exited {:?}: {}",
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
