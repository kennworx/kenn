//! Driver contract: the traits and value types every language indexer
//! shares, plus the stderr-capture helper their subprocess outcomes carry.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::canonicalize::Workspace;
use crate::docker::ContainerMount;
use crate::report::RunReport;

/// Translate an absolute host-path argument for a containerized indexer.
/// Under the Windows docker `Translate` mount (`mount` is `Some`) the path — the
/// workspace root or a descendant — is mapped to its `/work` container path;
/// otherwise (`None`: local runtime, or the POSIX same-path mount) the host path
/// passes through unchanged. Every driver routes its absolute path args through
/// this so a containerized indexer sees paths valid inside the container.
pub(crate) fn container_arg(mount: Option<&ContainerMount>, path: &Path) -> OsString {
    match mount {
        Some(m) => OsString::from(m.to_container(path)),
        None => path.as_os_str().to_owned(),
    }
}

/// Background reader for a child process's stderr. Drains the pipe into an
/// in-memory buffer so it can be attached to the `RunReport` if the child
/// exits non-zero. Reading prevents the child from blocking when its stderr
/// buffer fills (`kenn-dotnet` logs INFO lines + a multi-KiB stack trace on
/// fatal `MSBuild` errors).
pub struct StderrCapture {
    pub handle: JoinHandle<()>,
    pub buffer: Arc<Mutex<Vec<u8>>>,
}

impl StderrCapture {
    /// Tail of the captured stderr, lossily decoded as UTF-8. Caps at
    /// `max_bytes` from the end so the tail is bounded in `RunReport`.
    #[must_use]
    pub fn tail(&self, max_bytes: usize) -> String {
        let buf = self.buffer.lock().expect("stderr buffer poisoned");
        let start = buf.len().saturating_sub(max_bytes);
        String::from_utf8_lossy(buf.get(start..).unwrap_or(&[])).into_owned()
    }
}

/// One SCIP compilation unit. Produced by `ScipDriver::discover_units`.
#[derive(Debug, Clone)]
pub struct Unit {
    /// Workspace-relative identifier (e.g. `MyApp.sln`, `Cargo.toml`).
    pub identifier: String,
    /// Absolute path on disk; some drivers feed it to a subprocess.
    pub path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("indexer subprocess failed: {0}")]
    Subprocess(String),
    #[error("indexer not on PATH: {0}")]
    Unavailable(String),
}

/// Outcome of a SCIP driver's per-unit run.
pub enum ScipOutcome {
    Scip { path: PathBuf, report: RunReport },
    Unavailable { report: RunReport },
}

impl std::fmt::Debug for ScipOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scip { path, report } => f
                .debug_struct("Scip")
                .field("path", path)
                .field("report", report)
                .finish(),
            Self::Unavailable { report } => f
                .debug_struct("Unavailable")
                .field("report", report)
                .finish(),
        }
    }
}

/// Outcome of a JSONL indexer's whole-workspace run.
///
/// `stream_path` points at a file the producer is writing into (its stdout
/// was redirected to this file at spawn). The pipeline reads from the file
/// concurrently while the producer writes — file-backed handoff avoids the
/// OS pipe back-pressure that blocked walker threads on stdout writes
/// when the rust consumer was mid-flush.
pub enum JsonlOutcome {
    Jsonl {
        child: Child,
        stream_path: PathBuf,
        stderr: Option<StderrCapture>,
        report: RunReport,
    },
    Unavailable {
        report: RunReport,
    },
}

impl std::fmt::Debug for JsonlOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Jsonl {
                report,
                stream_path,
                ..
            } => f
                .debug_struct("Jsonl")
                .field("child", &"<Child>")
                .field("stream_path", stream_path)
                .field("stderr", &"<StderrCapture>")
                .field("report", report)
                .finish(),
            Self::Unavailable { report } => f
                .debug_struct("Unavailable")
                .field("report", report)
                .finish(),
        }
    }
}

/// Per-unit SCIP indexer. Pipeline calls `discover_units` once and
/// `run_unit` per unit.
pub trait ScipDriver: Send + Sync {
    fn language_id(&self) -> &str;
    /// The CLI command this driver spawns — checked by the phase-1
    /// preflight before any store write.
    fn command(&self) -> PathBuf;
    fn discover_units(&self, workspace: &Workspace) -> Result<Vec<Unit>, DriverError>;
    fn run_unit(&self, unit: &Unit, workspace: &Workspace) -> Result<ScipOutcome, DriverError>;
    /// The Windows docker `Translate` mount, when this driver runs its indexer
    /// in a container that reports `project_root` as `/work`. `None` (the
    /// default, and every non-`Translate` run) means the reported `project_root`
    /// is the real host path and MUST NOT be reconciled — a genuine
    /// `project_root`/workspace mismatch is a real out-of-root signal to preserve.
    fn container_mount(&self) -> Option<&ContainerMount> {
        None
    }
}

/// Whole-workspace JSONL streaming indexer. Pipeline calls `run` exactly
/// once per workspace; the indexer decides what to index and how to
/// schedule it.
pub trait JsonlIndexer: Send + Sync {
    fn language_id(&self) -> &str;
    /// The CLI command this indexer spawns — checked by the phase-1
    /// preflight before any store write.
    fn command(&self) -> PathBuf;
    fn run(&self, workspace: &Workspace) -> Result<JsonlOutcome, DriverError>;
}

pub(crate) fn spawn_stderr_capture(mut pipe: std::process::ChildStderr) -> StderrCapture {
    use std::io::Read;
    let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
    let buf_clone = buffer.clone();
    let handle = std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n @ 1..) = pipe.read(&mut chunk) {
            if let (Ok(mut b), Some(slice)) = (buf_clone.lock(), chunk.get(..n)) {
                b.extend_from_slice(slice);
            }
        }
    });
    StderrCapture { handle, buffer }
}

/// A one-line, human-actionable reason distilled from a failed producer's
/// stderr, for the `RunReport`'s `failed_projects`.
///
/// `stderr.lines().last()` is a trap: rust-analyzer / cargo failures end with a
/// panic backtrace, so the last line is a frame like `6: __pthread_cond_wait`
/// and the real cause (`error: current package believes it's in a workspace
/// when it's not`) is thrown away. Prefer the first line beginning with `error`
/// — the root-cause line these tools emit — and only fall back to the last
/// non-empty line when there is none.
pub(crate) fn error_reason(stderr: &str) -> &str {
    let is_error_line = |l: &&str| l.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("error"));
    stderr
        .lines()
        .map(str::trim)
        .find(is_error_line)
        .or_else(|| stderr.lines().map(str::trim).rev().find(|l| !l.is_empty()))
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::error_reason;

    #[test]
    fn prefers_the_error_line_over_a_trailing_backtrace_frame() {
        // rust-analyzer's real failure shape: the actionable line is mid-stream
        // and stderr ends with a panic backtrace frame. `lines().last()` would
        // return the frame; `error_reason` must return the `error:` line.
        let stderr = "Generating SCIP start...\n\
             error: current package believes it's in a workspace when it's not:\n\
             note: add an empty [workspace] table\n\
             stack backtrace:\n   6: __pthread_cond_wait";
        assert_eq!(
            error_reason(stderr),
            "error: current package believes it's in a workspace when it's not:"
        );
    }

    #[test]
    fn skips_an_internal_log_line_that_merely_contains_error() {
        // A timestamped `... ERROR ...` log line does not *begin* with error, so
        // it must not shadow the clean `error:` line that follows.
        let stderr = "2026-07-11 ERROR internal: failed fetching root\n\
             error: the real cause";
        assert_eq!(error_reason(stderr), "error: the real cause");
    }

    #[test]
    fn falls_back_to_the_last_nonempty_line_when_no_error_line() {
        assert_eq!(error_reason("warming up\nall done\n\n"), "all done");
    }
}
