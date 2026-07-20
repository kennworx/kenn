//! Atomic `live` symlink flip and directory-fsync helpers.

use std::fs::{self, File};
use std::path::Path;

/// Atomically flip the `live` symlink to point at `target` (an
/// absolute path inside `local/runs/`). The relative target is
/// computed against the symlink's own directory, so the store stays
/// relocatable.
pub(super) fn atomic_flip_live(live: &Path, target: &Path) -> std::io::Result<()> {
    let base = live.parent().unwrap_or(Path::new("."));
    let relative = target
        .strip_prefix(base)
        .map_or_else(|_| target.to_path_buf(), Path::to_path_buf);
    let tmp = base.join(format!("live.tmp.{}", std::process::id()));
    if tmp.exists() {
        fs::remove_file(&tmp)?;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&relative, &tmp)?;
    }
    #[cfg(not(unix))]
    {
        return Err(std::io::Error::other(
            "atomic flip is POSIX-only in v1; see `index-lifecycle` §Atomic flip portability",
        ));
    }
    // POSIX `rename(2)` is atomic; it replaces the existing symlink.
    fs::rename(&tmp, live)?;
    fsync_dir(base)?;
    Ok(())
}

pub(super) fn fsync_dir(path: &Path) -> std::io::Result<()> {
    // Best-effort: open as read-only and call sync_all. Some platforms
    // don't permit fsync on directories — swallow EINVAL/EPERM rather
    // than fail.
    let Ok(f) = File::open(path) else {
        return Ok(());
    };
    if let Err(e) = f.sync_all() {
        if matches!(
            e.kind(),
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::PermissionDenied
        ) {
            return Ok(());
        }
        return Err(e);
    }
    Ok(())
}
