//! Atomic PID-file write + stale-PID detection.
//!
//! Writes are atomic: write to `<file>.tmp`, fsync, rename. The
//! authoritative source of truth for `kenn server stop` and `kenn
//! server status`.

use std::io::Write as _;
use std::path::Path;

use crate::ServerError;

/// Write `pid` to `path` atomically.
pub fn write(path: &Path, pid: u32) -> Result<(), ServerError> {
    let tmp = path.with_extension("pid.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        writeln!(f, "{pid}")?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Read the PID from `path`. Returns `Ok(None)` when the file is
/// absent, `Err` on read or parse failure.
pub fn read(path: &Path) -> Result<Option<u32>, ServerError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let s = text.trim();
    s.parse::<u32>()
        .map(Some)
        .map_err(|e| ServerError::Other(format!("parse pid `{s}`: {e}")))
}

/// Remove the PID file. Missing file is not an error.
pub fn remove(path: &Path) -> Result<(), ServerError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Is the process with this PID currently alive (and accessible to the
/// current user)?
///
/// - Unix: `kill(pid, 0)` returns `Ok(())` if the process exists AND
///   we have permission to signal it; `Err(ESRCH)` if it doesn't
///   exist; `Err(EPERM)` if it exists but is owned by another user.
///   We treat `EPERM` as "alive but not ours" → also `true` so we
///   don't silently kill an unrelated PID, but the caller can decide.
/// - Windows: `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE,
///   pid)` succeeds iff the process exists.
#[must_use]
#[cfg(unix)]
pub fn is_alive(pid: u32) -> bool {
    let Ok(pid_i) = i32::try_from(pid) else {
        return false;
    };
    let pid = nix::unistd::Pid::from_raw(pid_i);
    // `kill(pid, None)` is a no-op existence probe: 0 sent, no signal
    // is delivered. EPERM means the process exists but isn't ours —
    // we still report "alive" so the caller doesn't silently kill an
    // unrelated PID.
    matches!(
        nix::sys::signal::kill(pid, None),
        Ok(()) | Err(nix::errno::Errno::EPERM)
    )
}

#[must_use]
#[cfg(windows)]
pub fn is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // Safety: OpenProcess returns 0 on failure; we don't dereference.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle == 0 {
        return false;
    }
    // Safety: handle is a valid HANDLE just returned by OpenProcess.
    unsafe { CloseHandle(handle) };
    true
}

#[cfg(test)]
mod tests {
    use super::{is_alive, read, remove, write};

    #[test]
    fn write_then_read_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("server.pid");
        write(&p, 12345).unwrap();
        assert_eq!(read(&p).unwrap(), Some(12345));
    }

    #[test]
    fn read_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("absent.pid");
        assert_eq!(read(&p).unwrap(), None);
    }

    #[test]
    fn remove_missing_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("absent.pid");
        remove(&p).unwrap();
    }

    #[test]
    fn write_overwrites_previous_value() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("server.pid");
        write(&p, 1).unwrap();
        write(&p, 2).unwrap();
        assert_eq!(read(&p).unwrap(), Some(2));
    }

    #[test]
    fn read_with_trailing_whitespace_parses() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("server.pid");
        std::fs::write(&p, "  42  \n").unwrap();
        assert_eq!(read(&p).unwrap(), Some(42));
    }

    #[test]
    fn read_with_bogus_content_errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("server.pid");
        std::fs::write(&p, "not-a-pid").unwrap();
        read(&p).unwrap_err();
    }

    #[test]
    fn current_process_is_alive() {
        let me = std::process::id();
        assert!(is_alive(me));
    }

    #[test]
    fn unlikely_pid_is_not_alive() {
        // An extremely high pid that's vanishingly unlikely to be in use.
        assert!(!is_alive(4_000_000_000));
    }
}
