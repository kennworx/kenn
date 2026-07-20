//! Per-OS state-directory resolution for the daemon's PID file and log and the
//! `cc-hook` collector store.
//!
//! - Unix (Linux **and** macOS): `$XDG_STATE_HOME/kenn` when set, else
//!   `~/.local/state/kenn/`. macOS uses the XDG-style path too — it does **not**
//!   use `~/Library/Application Support/kenn/`.
//! - Windows: `%LOCALAPPDATA%\kenn\` (via the `directories` crate).

use std::path::PathBuf;

use crate::ServerError;

/// The per-OS state directory for kenn's daemon and the `cc-hook` collector.
/// Returns `None` only when no home directory can be resolved for the current
/// user.
///
/// Resolution order: `$KENN_STATE_DIR` (test override) → `$XDG_STATE_HOME/kenn`
/// → `~/.local/state/kenn` on Unix (Linux and macOS alike) / the `directories`
/// state-or-data dir on other platforms.
#[must_use]
pub fn state_dir() -> Option<PathBuf> {
    if let Some(override_) = std::env::var_os("KENN_STATE_DIR") {
        return Some(PathBuf::from(override_));
    }
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("kenn"));
        }
    }
    #[cfg(unix)]
    {
        // Linux *and* macOS: the XDG-style path. macOS deliberately does not
        // use `~/Library/Application Support/kenn/`.
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".local/state/kenn"))
    }
    #[cfg(not(unix))]
    {
        let pd = directories::ProjectDirs::from("", "", "kenn")?;
        Some(
            pd.state_dir()
                .unwrap_or_else(|| pd.data_local_dir())
                .to_path_buf(),
        )
    }
}

/// Resolve the state directory, creating it if missing. Bubbles a
/// `ServerError::NoStateDir` when the OS doesn't expose a path.
pub fn ensure_state_dir() -> Result<PathBuf, ServerError> {
    let dir = state_dir().ok_or(ServerError::NoStateDir)?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Path to the daemon's PID file: `<state_dir>/server.pid`.
pub fn pid_file() -> Result<PathBuf, ServerError> {
    Ok(ensure_state_dir()?.join("server.pid"))
}

/// Path to the daemon's log file: `<state_dir>/server.log`.
pub fn log_file() -> Result<PathBuf, ServerError> {
    Ok(ensure_state_dir()?.join("server.log"))
}

#[cfg(test)]
mod tests {
    use super::state_dir;

    #[test]
    fn state_dir_resolves_on_this_platform() {
        // The env overrides (set by other in-process tests / the ambient
        // shell) would change the base path; guard against them so the
        // suffix assertion holds. The override-wins behavior is covered
        // end-to-end by the subprocess-based cc_hook_smoke tests.
        if std::env::var_os("KENN_STATE_DIR").is_some()
            || std::env::var_os("XDG_STATE_HOME").is_some()
        {
            return;
        }
        let dir = state_dir().expect("platform should expose a state dir");
        assert!(
            dir.components().any(|c| c.as_os_str() == "kenn"),
            "state dir {dir:?} should contain a `kenn` segment"
        );
        // On Unix (Linux and macOS) it is the XDG-style path, never
        // `~/Library/Application Support`.
        #[cfg(unix)]
        {
            assert!(
                dir.ends_with(".local/state/kenn"),
                "unix state dir {dir:?} should be ~/.local/state/kenn"
            );
            assert!(
                !dir.to_string_lossy().contains("Application Support"),
                "macOS must not use ~/Library/Application Support: {dir:?}"
            );
        }
    }
}
