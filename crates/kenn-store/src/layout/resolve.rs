//! Path-resolution helpers used by [`Layout::resolve`]: the relative /
//! absolute / `"global"` location-spec parser, the same-filesystem
//! probe that caches the writer tmp dir choice, and the XDG-cache
//! global-cache resolver.

use std::fs;
use std::path::{Path, PathBuf};

use xxhash_rust::xxh3::xxh3_64;

use super::types::StoreError;

/// `YYYY-MM-DDTHH-MM-SSZ` — the locked run-id timestamp format
/// (same shape `lifecycle.rs` uses for snapshot ids).
pub(super) const RUN_ID_FORMAT: &[time::format_description::FormatItem<'static>] =
    time::macros::format_description!("[year]-[month]-[day]T[hour]-[minute]-[second]Z");

/// Resolve a layout-style `Option<&str>` spec to a concrete path.
///
/// - `None` → `default_path.clone()`.
/// - `Some("global")` → `<XDG cache home>/<cache_top>/<project_id>`,
///   where `project_id = xxh3_64(canonicalize(source_root))`.
///   `cache_top` differs between roots (`"kenn"` for derived,
///   `"kenn-vectors"` for vectors) so the two `"global"` resolutions
///   don't collide.
/// - `Some(abs_path)` → `PathBuf::from(abs_path)`.
/// - `Some(rel_path)` → `anchor_root.join(rel_path)`. The anchor is the
///   source root for the per-worktree derived store, but the repo's
///   **main worktree** for vectors (`shared-vector-cache` Phase 1) — so
///   every linked worktree's relative `[vectors] location` lands in one
///   shared dir. The `"global"` arm stays keyed by `source_root`
///   (superseded by the shared default, not worth a keying migration).
pub(super) fn resolve_location_spec(
    spec: Option<&str>,
    source_root: &Path,
    anchor_root: &Path,
    cache_top: &str,
    default_path: &Path,
) -> Result<PathBuf, StoreError> {
    match spec {
        None => Ok(default_path.to_path_buf()),
        Some("global") => global_cache_root(source_root, cache_top),
        Some(s) => {
            let p = PathBuf::from(s);
            Ok(if p.is_absolute() {
                p
            } else {
                anchor_root.join(p)
            })
        }
    }
}

/// Walk up to the nearest existing ancestor of `path`, return its
/// `stat.dev`. The starting path may not yet exist (e.g.,
/// `[vectors] location` set to a directory we haven't created yet),
/// so we look at the closest existing parent — device id is a mount
/// property, so any existing ancestor on the same mount gives the
/// right answer (per design D8).
///
/// Returns `None` only if no ancestor up to `/` exists, which would
/// indicate a pathologically broken filesystem.
fn ancestor_device_id(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let mut cur: Option<&Path> = Some(path);
    while let Some(p) = cur {
        if let Ok(meta) = fs::metadata(p) {
            return Some(meta.dev());
        }
        cur = p.parent();
    }
    None
}

/// `true` when `a` and `b` resolve to the same filesystem (same
/// `stat.dev` on their nearest existing ancestors). Used at
/// `Layout::resolve()` time to cache the writer-tmp-dir choice (D8).
/// Returns `true` defensively when either device id cannot be
/// determined; the worst case is a misclassification that surfaces
/// as `EXDEV` at first write, which is louder than a silent
/// cross-fs fallback.
pub(super) fn same_filesystem(a: &Path, b: &Path) -> bool {
    match (ancestor_device_id(a), ancestor_device_id(b)) {
        (Some(da), Some(db)) => da == db,
        _ => true,
    }
}

/// Resolve a `"global"` location spec to an XDG-cache path keyed by
/// a stable per-repository project id (the xxh3-64 of the canonicalized
/// source root). `cache_top` separates the namespaces of the two
/// independent `"global"` knobs (`"kenn"` for `[layout] derived_root`,
/// `"kenn-vectors"` for `[vectors] location`), so the two never
/// collide even when both are set to `"global"`.
fn global_cache_root(source_root: &Path, cache_top: &str) -> Result<PathBuf, StoreError> {
    let cache_home = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or_else(|| {
            StoreError::Config(
                "`\"global\"` location needs XDG_CACHE_HOME or HOME to be set".to_owned(),
            )
        })?;
    let canonical = fs::canonicalize(source_root).unwrap_or_else(|_| source_root.to_path_buf());
    let project_id = format!("{:016x}", xxh3_64(canonical.as_os_str().as_encoded_bytes()));
    Ok(cache_home.join(cache_top).join(project_id))
}
