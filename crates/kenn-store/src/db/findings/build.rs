//! The findings-publish lock.
//!
//! The Lance mirror build / catch-up / embed paths were removed when the
//! findings store became records-based (replace-lance-with-sqlite); only the
//! cross-process publish lock that fences `store_finding`'s flush against a
//! reindex's `live` flip remains.

use crate::api::types::DbError;
use crate::layout::Layout;

/// Acquire the workspace-wide findings-publish lock — a blocking POSIX
/// `flock`. Dropping the returned file releases it. Cross-process and CLI-safe.
pub fn acquire_findings_publish_lock(layout: &Layout) -> Result<std::fs::File, DbError> {
    let lock_path = layout.findings_publish_lock_path();
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(DbError::Io)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| DbError::Backend(format!("open publish lock: {e}")))?;
    fs2::FileExt::lock_exclusive(&file)
        .map_err(|e| DbError::Backend(format!("acquire publish lock: {e}")))?;
    Ok(file)
}
