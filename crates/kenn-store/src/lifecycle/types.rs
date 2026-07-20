//! Lifecycle types — the run-state machine view, the indexing-handle
//! struct, the error enums, and the recovery / startup reports.

use std::fs::File;
use std::path::PathBuf;

use thiserror::Error;

use crate::layout::{Store, StoreError};

/// The completion-stamp file written inside a published run directory.
/// Presence ⇒ run is complete and eligible to be the `live` target.
pub(super) const META_FILE: &str = "meta.json";

/// Marker file inside a run directory whose mtime records the run's
/// last-access time, driving LRU [`crate::lifecycle::gc`]. Run datasets
/// are immutable; only this metadata marker is rewritten.
pub(super) const ACCESS_MARKER: &str = ".accessed";

/// Where the lifecycle is right now. Computed from the filesystem
/// (`live` symlink + `runs/` contents); no in-memory state caches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleState {
    Uninitialized,
    Steady {
        live: PathBuf,
    },
    Indexing {
        live: Option<PathBuf>,
        run_dir: PathBuf,
    },
}

#[derive(Debug, Error)]
pub enum BeginError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("another indexer is already running on this workspace (lock {0:?})")]
    LockHeld(PathBuf),
}

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "run directory missing `meta.json` — the pipeline did not complete the run before publish"
    )]
    NoMeta,
}

#[derive(Debug, Error)]
pub enum RollbackError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("no previous run retained")]
    NoPrevious,
    #[error("`live` symlink missing — nothing to roll back from")]
    NoLive,
}

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Handle returned by [`crate::lifecycle::begin_indexing`]. Owns the
/// exclusive flock for the duration of the run; dropping without
/// calling `publish`/`abort` deletes the run directory and releases
/// the lock (best-effort).
#[derive(Debug)]
pub struct IndexingHandle {
    pub(super) store: Store,
    pub(super) run_dir: PathBuf,
    pub(super) lock_file: Option<File>,
    pub(super) finalized: bool,
}

/// What the MCP server (or another startup-time caller) should do based
/// on the current store state and the workspace's freshness check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupDecision {
    /// A live run exists and (per the staleness check) is fresh
    /// enough to serve from. The caller should open it read-only and
    /// skip indexing.
    Skip { live: PathBuf },
    /// The caller should run a full reindex. `reason` is a
    /// human-readable short string suitable for logs / progress
    /// messages.
    Reindex { reason: &'static str },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Paths of run directories that lacked `meta.json` and were
    /// deleted during recovery.
    pub deleted_incomplete_runs: Vec<PathBuf>,
    /// Count of `*.tmp` files removed from the cross-fs fallback
    /// tmp dir (§5.7). Always 0 in the common case where vectors and
    /// derived roots share a filesystem.
    pub swept_cross_fs_tmp_files: usize,
}
