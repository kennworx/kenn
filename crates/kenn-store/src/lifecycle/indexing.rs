//! `begin_indexing` and the [`IndexingHandle`] lifecycle methods.

use std::fs::{self, File};
use std::path::Path;
use std::path::PathBuf;

use fs2::FileExt;
use time::OffsetDateTime;

use crate::layout::{Layout, Store};

use super::atomic::{atomic_flip_live, fsync_dir};
use super::types::{BeginError, IndexingHandle, PublishError, META_FILE};

/// Begin an indexing run.
///
/// - Acquires an exclusive flock on `<derived_root>/index.lock`
///   (non-blocking).
/// - Picks a fresh run id (ISO-8601 UTC second-precision, with `-N`
///   suffix on same-second collisions — see
///   [`Layout::new_run_id`]).
/// - Creates `runs/{run_id}/` and a per-run `tmp/` for atomic-rename
///   sidecar writes.
///
/// Per D1, the run directory IS the published directory once
/// `meta.json` is written into it — no separate `building/` rename
/// step at publish time.
pub fn begin_indexing(store: &Store) -> Result<IndexingHandle, BeginError> {
    fs::create_dir_all(store.local_dir())?;
    let lock_path = store.lock_path();
    let lock_file = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    if FileExt::try_lock_exclusive(&lock_file).is_err() {
        return Err(BeginError::LockHeld(lock_path));
    }

    // Pick a unique run id by consulting the existing runs.
    let last_id = store
        .list_runs()
        .ok()
        .and_then(|mut v| v.pop())
        .map(|m| m.id);
    let run_id = Layout::new_run_id(OffsetDateTime::now_utc(), last_id.as_deref())
        .map_err(BeginError::Store)?;
    let run_dir = store.run_dir(&run_id);
    if run_dir.exists() {
        drop(FileExt::unlock(&lock_file));
        return Err(BeginError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "run directory already exists at {} — race against another indexer?",
                run_dir.display()
            ),
        )));
    }
    fs::create_dir_all(&run_dir)?;
    // Per-run tmp dir for sidecar writers (D8). The vectors writer may
    // also use a cross-fs fallback dir; that is created lazily by the
    // writer itself.
    fs::create_dir_all(run_dir.join("tmp"))?;

    Ok(IndexingHandle {
        store: store.clone(),
        run_dir,
        lock_file: Some(lock_file),
        finalized: false,
    })
}

impl IndexingHandle {
    /// The run directory the pipeline writes into. The `SQLite` databases
    /// (`code.db`, `vector.db`) and per-language SCIP/JSONL inputs go
    /// at the top of the run dir; `meta.json` (the completion stamp) is
    /// written last.
    #[must_use]
    pub fn run_dir(&self) -> &Path {
        &self.run_dir
    }

    /// Publish the run: verify `meta.json` is present (the pipeline's
    /// completion stamp), fsync the run dir, and atomically flip
    /// `live` to it via tmp-symlink + rename(2).
    ///
    /// No directory rename — the run dir IS the published dir (D1).
    pub fn publish(mut self) -> Result<PathBuf, PublishError> {
        if !self.run_dir.join(META_FILE).is_file() {
            return Err(PublishError::NoMeta);
        }
        fsync_dir(&self.run_dir)?;
        atomic_flip_live(&self.store.live_path(), &self.run_dir)?;
        self.finalized = true;
        Ok(self.run_dir.clone())
    }

    /// Discard the run without publishing — removes the run
    /// directory; `live` is unchanged. Lock released on drop.
    pub fn abort(mut self) -> Result<(), std::io::Error> {
        if self.run_dir.exists() {
            fs::remove_dir_all(&self.run_dir)?;
        }
        self.finalized = true;
        Ok(())
    }
}

impl Drop for IndexingHandle {
    fn drop(&mut self) {
        if !self.finalized && self.run_dir.exists() {
            // D1 invariant: `meta.json` presence stamps the run as
            // complete. A handle dropped without `publish()`/`abort()`
            // — typically a panic, but also a `publish()` that wrote
            // meta then errored on fsync/symlink-flip — must retain
            // the run if meta.json is present. Otherwise we'd nuke a
            // complete run whose only failure was the `live` flip,
            // losing all its data. `recover()` on next start
            // distinguishes incomplete (no meta) from complete (has
            // meta) and acts accordingly.
            let has_meta = self.run_dir.join(META_FILE).is_file();
            if !has_meta {
                // Incomplete — best-effort cleanup; ignore errors.
                // `recover()` on next start would catch any leftover.
                drop(fs::remove_dir_all(&self.run_dir));
            }
        }
        if let Some(f) = self.lock_file.take() {
            drop(FileExt::unlock(&f));
        }
    }
}
