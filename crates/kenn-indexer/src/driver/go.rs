use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::canonicalize::Workspace;
use crate::report::{RunReport, RunStatus};

use kenn_model::Language;

use super::{
    error_reason, make_scip_output_path, walk_for_language, DriverError, ScipDriver, ScipOutcome,
    Unit,
};

/// scip-go SCIP driver. Spawns `scip-go index --module-root <dir>
/// --output <out> --quiet` once per discovered Go module (`go.mod`).
///
/// scip-go is module-scoped: `--module-root` points at a single
/// `go.mod`. A monorepo with several modules yields one unit per
/// `go.mod` (cf. dotnet's one-unit-per-`.sln`). scip-go loads the module
/// through `go/packages`, so the module must already be BUILT — kenn
/// does not run `go build`/`go mod download` (same hands-off posture as
/// rust-analyzer/Swift). A cold module makes scip-go appear to hang
/// while `go/packages` compiles missing dependencies on the fly; once
/// the build cache is warm scip-go is fast even on large repos.
pub struct ScipGo {
    /// Launcher tokens — `command[0]` is the program (subject to the
    /// Tier-2 PATH probe), `command[1..]` are leading arguments
    /// prepended to the trailing `index ...` args. Defaults to
    /// `["scip-go"]`; users override with e.g. `["/opt/go/bin/scip-go"]`.
    pub command: Vec<String>,
}

impl Default for ScipGo {
    fn default() -> Self {
        Self {
            command: vec!["scip-go".into()],
        }
    }
}

impl ScipDriver for ScipGo {
    fn language_id(&self) -> &'static str {
        "go"
    }

    fn command(&self) -> PathBuf {
        PathBuf::from(self.command.first().map_or("scip-go", String::as_str))
    }

    fn discover_units(&self, workspace: &Workspace) -> Result<Vec<Unit>, DriverError> {
        // walk_for_language prunes `[workspace].excludes` AND
        // `[language.go].excludes` (vendor/, testdata/) at recursion
        // time, so a `go.mod` under vendored deps or test fixtures never
        // becomes its own unit. One unit per remaining `go.mod`, rooted
        // at the directory that contains it.
        let mut units = Vec::new();
        for entry in walk_for_language(workspace, Language::Go) {
            let entry = entry?;
            if entry.file_name().and_then(|s| s.to_str()) == Some("go.mod") {
                if let Some(dir) = entry.parent() {
                    units.push(Unit {
                        identifier: format!("go-{}", units.len()),
                        path: dir.to_path_buf(),
                    });
                }
            }
        }
        Ok(units)
    }

    fn run_unit(&self, unit: &Unit, workspace: &Workspace) -> Result<ScipOutcome, DriverError> {
        let program = self.command.first().map_or("scip-go", String::as_str);
        let output = make_scip_output_path(workspace, &unit.identifier)?;
        let mut report =
            RunReport::started_for(kenn_model::Language::Go, "scip-go", "?", &unit.identifier);

        let mut cmd = Command::new(program);
        cmd.args(self.command.iter().skip(1));
        cmd.arg("index")
            .arg("--module-root")
            .arg(&unit.path)
            .arg("--output")
            .arg(&output)
            .arg("--quiet");
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let out = match cmd.output() {
            Ok(o) => o,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.status = RunStatus::Failed;
                report
                    .failed_projects
                    .push(format!("scip-go launcher `{program}` not found on PATH"));
                report.finalize();
                return Ok(ScipOutcome::Unavailable { report });
            }
            Err(e) => return Err(e.into()),
        };
        if !out.status.success() {
            report.status = RunStatus::Failed;
            let stderr = String::from_utf8_lossy(&out.stderr);
            report.failed_projects.push(format!(
                "scip-go exited {:?}: {}",
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
