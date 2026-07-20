//! Cross-process reader registry — pins snapshot directories against
//! [`crate::lifecycle::gc`] collection while any process is reading from
//! them.
//!
//! Each holding process flock-locks a `<pid>` marker file under
//! `<readers_dir>/<snapshot-id>/`. GC, before evicting a snapshot,
//! probes that snapshot's markers with a non-blocking exclusive `flock`:
//! a successful lock means the previous holder is dead (kernel released
//! the lock on process exit) and the marker is reclaimed; a contended
//! lock means a live reader holds the snapshot, which is then skipped.
//! Snapshot directories themselves stay immutable — the registry lives
//! in a sibling tree, never inside the snapshot.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fs2::FileExt;
use thiserror::Error;

use crate::layout::Store;

/// Per-process counter appended to every marker filename so the same
/// process can register multiple non-conflicting markers for one
/// snapshot — needed by tests that simulate two instances in one
/// process; production processes typically register once per snapshot.
static MARKER_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum ReaderRegistryError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("snapshot path has no file name: {0}")]
    UnnamedSnapshot(PathBuf),
}

/// RAII guard for a reader's snapshot pin. Dropping it releases the
/// `flock` and removes the marker file (best-effort); even if removal
/// fails, the next GC sweep reclaims the dead marker.
#[derive(Debug)]
pub struct ReaderMarker {
    path: PathBuf,
    file: Option<File>,
}

impl ReaderMarker {
    /// The on-disk marker file path — mostly for diagnostics and tests.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ReaderMarker {
    fn drop(&mut self) {
        if let Some(f) = self.file.take() {
            drop(FileExt::unlock(&f));
        }
        // Best-effort removal — a leftover file is reclaimed by the next
        // GC sweep's probe.
        drop(fs::remove_file(&self.path));
    }
}

/// Register the current process as a reader of `snapshot_dir`, creating
/// and `flock`-locking `<readers_dir>/<snapshot-id>/<pid>`. The snapshot
/// id is the snapshot directory's leaf name. Returns a guard whose drop
/// releases the pin.
pub fn register_reader(
    store: &Store,
    snapshot_dir: &Path,
) -> Result<ReaderMarker, ReaderRegistryError> {
    let snapshot_id = snapshot_dir
        .file_name()
        .ok_or_else(|| ReaderRegistryError::UnnamedSnapshot(snapshot_dir.to_path_buf()))?;
    let dir = store.readers_dir().join(snapshot_id);
    fs::create_dir_all(&dir)?;
    let pid = std::process::id();
    let seq = MARKER_COUNTER.fetch_add(1, Ordering::Relaxed);
    // `<pid>.<seq>` — pid identifies the holding process for diagnostics
    // and the dead-marker probe (no semantic role); seq lets one process
    // hold multiple non-conflicting flocks on the same snapshot. What
    // matters for the GC pin is the `flock` being held by SOME live
    // file descriptor in the registry — the filename is just storage.
    let path = dir.join(format!("{pid}.{seq}"));
    let file = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    FileExt::lock_exclusive(&file)?;
    Ok(ReaderMarker {
        path,
        file: Some(file),
    })
}

/// True iff any live process holds a reader marker for `snapshot_dir`.
///
/// Probes every marker file under the snapshot's registry entry with a
/// non-blocking exclusive `flock`: a successful lock means the holder
/// died (the kernel releases `flock`s on process exit) so the marker is
/// reclaimed; a contended lock means a live reader pins the snapshot.
/// A missing registry entry trivially returns `false`.
pub fn snapshot_has_live_reader(
    store: &Store,
    snapshot_dir: &Path,
) -> Result<bool, ReaderRegistryError> {
    let Some(snapshot_id) = snapshot_dir.file_name() else {
        return Ok(false);
    };
    let dir = store.readers_dir().join(snapshot_id);
    let entries = match fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let mut any_live = false;
    for entry in entries.flatten() {
        let path = entry.path();
        // Open writable for an exclusive flock; if the file vanished
        // since `read_dir`, just skip it.
        let Ok(file) = File::options().write(true).open(&path) else {
            continue;
        };
        if FileExt::try_lock_exclusive(&file).is_ok() {
            // Dead holder — release our test lock and reclaim the marker.
            drop(FileExt::unlock(&file));
            drop(fs::remove_file(&path));
        } else {
            any_live = true;
        }
    }
    // If we reclaimed the last marker, also drop the now-empty snapshot
    // directory under the registry so the tree does not leak.
    if !any_live {
        drop(fs::remove_dir(&dir));
    }
    Ok(any_live)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, Store) {
        let ws = TempDir::new().unwrap();
        let s = Store::open_default(ws.path()).unwrap();
        (ws, s)
    }

    fn fake_snapshot(store: &Store, name: &str) -> PathBuf {
        let p = store.runs_dir().join(name);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn register_creates_a_marker_under_snapshot_id() {
        let (_ws, s) = store();
        let snap = fake_snapshot(&s, "2026-01-01T00-00-00Z");
        let marker = register_reader(&s, &snap).unwrap();
        let expected_dir = s.readers_dir().join("2026-01-01T00-00-00Z");
        assert!(expected_dir.is_dir());
        // Marker filename is `<pid>.<seq>` — seq is process-local so we
        // don't assert the full leaf, just the parent dir and prefix.
        let leaf = marker.path().file_name().unwrap().to_str().unwrap();
        let pid_prefix = format!("{}.", std::process::id());
        assert!(
            leaf.starts_with(&pid_prefix),
            "marker leaf {leaf} should start with `{pid_prefix}`"
        );
        assert_eq!(marker.path().parent().unwrap(), expected_dir);
    }

    #[test]
    fn same_process_can_hold_multiple_markers_for_one_snapshot() {
        // Tests share a process, and multi-instance integration tests
        // need two `register_reader` calls against one snapshot in one
        // process — they must not self-deadlock on `flock`.
        let (_ws, s) = store();
        let snap = fake_snapshot(&s, "2026-01-05T00-00-00Z");
        let a = register_reader(&s, &snap).unwrap();
        let b = register_reader(&s, &snap).unwrap();
        assert_ne!(a.path(), b.path());
        assert!(snapshot_has_live_reader(&s, &snap).unwrap());
    }

    #[test]
    fn probe_detects_live_marker_and_reclaims_dead_one() {
        let (_ws, s) = store();
        let snap = fake_snapshot(&s, "2026-01-02T00-00-00Z");
        // Live: register and keep the guard alive.
        let _live = register_reader(&s, &snap).unwrap();
        assert!(snapshot_has_live_reader(&s, &snap).unwrap());

        // Dead marker: create a sibling marker file with no flock holder.
        let stale_path = s.readers_dir().join("2026-01-02T00-00-00Z").join("999999");
        File::create(&stale_path).unwrap();
        // The live one still pins; the dead one is reclaimed in passing.
        assert!(snapshot_has_live_reader(&s, &snap).unwrap());
        assert!(!stale_path.exists(), "stale marker should be reclaimed");
    }

    #[test]
    fn dropping_the_guard_releases_the_pin() {
        let (_ws, s) = store();
        let snap = fake_snapshot(&s, "2026-01-03T00-00-00Z");
        {
            let _m = register_reader(&s, &snap).unwrap();
            assert!(snapshot_has_live_reader(&s, &snap).unwrap());
        }
        // Marker file gone, no live readers.
        assert!(!snapshot_has_live_reader(&s, &snap).unwrap());
    }

    #[test]
    fn missing_registry_entry_means_no_live_reader() {
        let (_ws, s) = store();
        let snap = fake_snapshot(&s, "2026-01-04T00-00-00Z");
        // Never registered → registry dir does not exist.
        assert!(!snapshot_has_live_reader(&s, &snap).unwrap());
    }
}
