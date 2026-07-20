//! Daemonization, stop, and status helpers used by the `kenn server`
//! CLI subcommand.
//!
//! On Unix, detachment is `setsid` only (no fork) — the parent
//! `kenn server start` already fork-exec'd this process and pointed its
//! stdio at `server.log`; forking again would break llama.cpp/Metal (see
//! [`daemonize`]). On Windows, the parent CLI spawns the child with
//! `DETACHED_PROCESS` flags — there's no special work to do here.

use std::path::Path;
use std::time::Duration;

use crate::{pid, ServerError};

/// Send SIGTERM to the PID at `pid_path`, wait for the process to exit
/// with a grace, then SIGKILL if still alive. Stale PID files are
/// detected (process not alive) and cleaned up with no signal sent.
///
/// Returns:
/// - `Ok(true)` — a running daemon was signalled and exited.
/// - `Ok(false)` — no daemon was running (no PID file, or stale file).
/// - `Err(_)` — i/o or signal error.
#[cfg(unix)]
pub fn stop(pid_path: &Path) -> Result<bool, ServerError> {
    let Some(pid_u) = load_running_pid_or_clean(pid_path)? else {
        return Ok(false);
    };
    let pid_n = pid_to_nix(pid_u)?;
    if signal_and_wait(pid_path, pid_n, pid_u, &TERM_PHASE)? {
        return Ok(true);
    }
    if signal_and_wait(pid_path, pid_n, pid_u, &KILL_PHASE)? {
        return Ok(true);
    }
    Err(ServerError::Other(format!(
        "process {pid_u} still alive after SIGKILL"
    )))
}

#[cfg(unix)]
struct StopPhase {
    signal: nix::sys::signal::Signal,
    wait: Duration,
    poll: Duration,
}

#[cfg(unix)]
const TERM_PHASE: StopPhase = StopPhase {
    signal: nix::sys::signal::Signal::SIGTERM,
    wait: Duration::from_secs(5),
    poll: Duration::from_millis(100),
};

#[cfg(unix)]
const KILL_PHASE: StopPhase = StopPhase {
    signal: nix::sys::signal::Signal::SIGKILL,
    wait: Duration::from_secs(2),
    poll: Duration::from_millis(50),
};

/// Deliver `phase.signal` to the process, wait up to `phase.wait` for
/// it to exit. On clean exit removes the PID file and returns `true`.
/// `false` means the process is still alive after the wait window.
#[cfg(unix)]
fn signal_and_wait(
    pid_path: &Path,
    pid_n: nix::unistd::Pid,
    pid_u: u32,
    phase: &StopPhase,
) -> Result<bool, ServerError> {
    send_signal(pid_n, pid_u, phase.signal)?;
    if wait_until_dead(pid_u, phase.wait, phase.poll) {
        pid::remove(pid_path)?;
        return Ok(true);
    }
    Ok(false)
}

/// Read the PID file; return the running PID, or `Ok(None)` for either
/// "no file" or "stale file (cleaned up)".
#[cfg(unix)]
fn load_running_pid_or_clean(pid_path: &Path) -> Result<Option<u32>, ServerError> {
    let Some(pid_u) = pid::read(pid_path)? else {
        return Ok(None);
    };
    if pid::is_alive(pid_u) {
        Ok(Some(pid_u))
    } else {
        pid::remove(pid_path)?;
        Ok(None)
    }
}

#[cfg(unix)]
fn pid_to_nix(pid_u: u32) -> Result<nix::unistd::Pid, ServerError> {
    let pid_i = i32::try_from(pid_u)
        .map_err(|e| ServerError::Other(format!("pid {pid_u} exceeds i32 range: {e}")))?;
    Ok(nix::unistd::Pid::from_raw(pid_i))
}

#[cfg(unix)]
fn send_signal(
    pid_n: nix::unistd::Pid,
    pid_u: u32,
    sig: nix::sys::signal::Signal,
) -> Result<(), ServerError> {
    nix::sys::signal::kill(pid_n, sig)
        .map_err(|e| ServerError::Other(format!("kill {sig:?} {pid_u}: {e}")))
}

/// Poll `is_alive` until the process is gone or the deadline expires.
/// Returns `true` if the process died within the window.
#[cfg(unix)]
fn wait_until_dead(pid_u: u32, total: Duration, interval: Duration) -> bool {
    let deadline = std::time::Instant::now() + total;
    while std::time::Instant::now() < deadline {
        if !pid::is_alive(pid_u) {
            return true;
        }
        std::thread::sleep(interval);
    }
    !pid::is_alive(pid_u)
}

#[cfg(windows)]
pub fn stop(pid_path: &Path) -> Result<bool, ServerError> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    let Some(pid_u) = pid::read(pid_path)? else {
        return Ok(false);
    };
    if !pid::is_alive(pid_u) {
        pid::remove(pid_path)?;
        return Ok(false);
    }
    // Safety: passing a u32 PID; OpenProcess returns 0 on failure.
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid_u) };
    if handle == 0 {
        return Err(ServerError::Other(format!("OpenProcess({pid_u}) failed")));
    }
    // Safety: handle is a valid HANDLE just returned by OpenProcess.
    let term_ok = unsafe { TerminateProcess(handle, 0) };
    // Safety: same handle.
    unsafe { CloseHandle(handle) };
    if term_ok == 0 {
        return Err(ServerError::Other(format!(
            "TerminateProcess({pid_u}) failed"
        )));
    }
    pid::remove(pid_path)?;
    Ok(true)
}

/// Summary returned by [`status`].
#[derive(Debug)]
pub struct Status {
    /// PID file location (always reported for the user's convenience).
    pub pid_path: std::path::PathBuf,
    /// Running PID if a daemon is alive; `None` for "not running" or
    /// stale-and-cleaned-up.
    pub pid: Option<u32>,
    /// Whether the `pid` returned `Some` and the file existed (i.e. a
    /// running daemon was observed). False after stale cleanup.
    pub running: bool,
    /// True when the PID file existed but pointed at a dead process —
    /// `status` cleaned it up.
    pub cleaned_stale: bool,
}

/// Read the PID file and probe whether the named process is alive.
/// Cleans up a stale file (PID dead) before returning.
pub fn status(pid_path: &Path) -> Result<Status, ServerError> {
    let pid_path_buf = pid_path.to_path_buf();
    let Some(pid_u) = pid::read(pid_path)? else {
        return Ok(Status {
            pid_path: pid_path_buf,
            pid: None,
            running: false,
            cleaned_stale: false,
        });
    };
    if pid::is_alive(pid_u) {
        Ok(Status {
            pid_path: pid_path_buf,
            pid: Some(pid_u),
            running: true,
            cleaned_stale: false,
        })
    } else {
        pid::remove(pid_path)?;
        Ok(Status {
            pid_path: pid_path_buf,
            pid: None,
            running: false,
            cleaned_stale: true,
        })
    }
}

/// Detach the current process into its own session so it outlives the
/// `kenn server start` invocation and its controlling terminal.
///
/// IMPORTANT: this does **not** fork. The parent `kenn server start`
/// already fork-*exec*'d us as a fresh process with stdio pointed at
/// `<state_dir>/server.log`, so all we owe is session detachment via
/// `setsid`. We must never fork-without-exec here: on macOS, llama.cpp +
/// Metal cannot create a compute context in a process that has forked
/// without a following exec (the `__THE_PROCESS_HAS_FORKED__…` guard).
/// The old double-fork `daemonize`-crate path tripped exactly that, so
/// every `/v1/embeddings` request 503'd with "create embedding context:
/// null reference from llama.cpp" while the in-process embedder worked.
///
/// The PID file is written by [`crate::host::Host::serve`] after the
/// listener binds, so it always points at this live process.
#[cfg(unix)]
pub fn daemonize() -> Result<(), ServerError> {
    // Don't pin the spawning cwd (e.g. a repo being indexed) for the
    // daemon's lifetime; the server resolves all its paths absolutely.
    std::env::set_current_dir("/").map_err(|e| ServerError::Daemon(format!("chdir /: {e}")))?;
    // New session + process group, no controlling terminal. The parent
    // already redirected stdio to server.log, so there are no FDs to
    // reattach here.
    nix::unistd::setsid().map_err(|e| ServerError::Daemon(format!("setsid: {e}")))?;
    Ok(())
}

#[cfg(not(unix))]
pub fn daemonize() -> Result<(), ServerError> {
    // No-op on non-Unix: detachment is the spawning parent's
    // responsibility (DETACHED_PROCESS spawn flag).
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{status, stop};

    #[test]
    fn stop_with_no_pid_file_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("server.pid");
        assert!(!stop(&p).unwrap());
    }

    #[test]
    fn stop_with_stale_pid_file_cleans_up_and_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("server.pid");
        std::fs::write(&p, "4000000000\n").unwrap(); // unlikely-to-exist PID
        assert!(!stop(&p).unwrap());
        assert!(!p.exists(), "stale pid file should be removed");
    }

    #[test]
    fn status_with_no_pid_file_reports_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("server.pid");
        let s = status(&p).unwrap();
        assert!(!s.running);
        assert!(s.pid.is_none());
        assert!(!s.cleaned_stale);
    }

    #[test]
    fn status_with_stale_pid_file_cleans_up_and_reports_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("server.pid");
        std::fs::write(&p, "4000000000\n").unwrap();
        let s = status(&p).unwrap();
        assert!(!s.running);
        assert!(s.cleaned_stale);
        assert!(!p.exists());
    }

    #[test]
    fn status_with_live_pid_reports_running() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("server.pid");
        std::fs::write(&p, std::process::id().to_string()).unwrap();
        let s = status(&p).unwrap();
        assert!(s.running);
        assert_eq!(s.pid, Some(std::process::id()));
        assert!(!s.cleaned_stale);
    }
}
