//! Atomic `live` pointer-file flip and directory-fsync helpers.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

/// Atomically flip the `live` pointer file to name `target` (an
/// absolute path inside `local/runs/`). `live` is a small text file
/// holding the target's path relative to its own directory, so the
/// store stays relocatable — one format on every platform, because a
/// symlink cannot be flipped unprivileged on Windows (D1).
pub(super) fn atomic_flip_live(live: &Path, target: &Path) -> std::io::Result<()> {
    let base = live.parent().unwrap_or(Path::new("."));
    let relative = target
        .strip_prefix(base)
        .map_or_else(|_| target.to_path_buf(), Path::to_path_buf);
    // The run-relative target is always ASCII (`runs/<ISO-timestamp>`), so
    // `to_str` never returns `None` in practice — it is a graceful guard, not
    // a reachable path. (A raw-bytes round-trip that would drop even the guard
    // needs `OsStr::from_encoded_bytes_unchecked`, which the workspace's
    // `deny(unsafe_code)` forbids.)
    let pointer = relative.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "live target path is not valid UTF-8",
        )
    })?;
    let tmp = base.join(format!("live.tmp.{}", std::process::id()));
    // Write + fsync the pointer into a temp file first (`File::create`
    // truncates any stale temp), so a crash between the rename and the
    // directory fsync can't leave `live` pointing at unflushed
    // (empty/truncated) content.
    {
        let mut f = File::create(&tmp)?;
        f.write_all(pointer.as_bytes())?;
        f.sync_all()?;
    }
    // Rename over `live`. `rename` is atomic and replaces the existing file
    // on POSIX; on Windows it maps to a replace-if-exists that can hit a
    // transient sharing violation (D6).
    rename_pointer(&tmp, live)?;
    fsync_dir(base)?;
    Ok(())
}

/// `fs::rename` with a bounded retry on a Windows sharing violation (D6).
/// A third party that opens `live` without `FILE_SHARE_DELETE` — antivirus,
/// an editor, an Explorer preview — makes the replace fail transiently. The
/// flip is idempotent (the temp file already holds the complete target and
/// nothing has been mutated), so the retry waits out a known external lock
/// rather than papering over an unknown failure. POSIX `rename` never
/// returns this code, so there the loop runs exactly once — one code path,
/// no `cfg` fork.
fn rename_pointer(tmp: &Path, live: &Path) -> std::io::Result<()> {
    /// Windows `ERROR_SHARING_VIOLATION`.
    const SHARING_VIOLATION: i32 = 32;
    let mut attempt = 0u32;
    loop {
        match fs::rename(tmp, live) {
            Ok(()) => return Ok(()),
            Err(e) if e.raw_os_error() == Some(SHARING_VIOLATION) && attempt < 5 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(u64::from(attempt) * 20));
            }
            Err(e) => return Err(e),
        }
    }
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
