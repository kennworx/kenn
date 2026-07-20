//! Run-and-flip lifecycle — `index-lifecycle` capability.
//!
//! State machine (D1 — runs ≡ snapshots):
//! ```text
//! Uninitialized ─────────┐
//!                        ▼
//!   Steady(R_n) ──► Indexing(R_n, R_{n+1}) ──► Steady(R_{n+1})
//!         ▲                                          │
//!         └────────────── rollback ──────────────────┘
//! ```
//!
//! Each `kenn index` pass writes directly into `runs/{id}/` — there is
//! no separate `building/` directory under the new layout. Completion
//! is stamped by writing `meta.json` into the run; a run with
//! `meta.json` is "published" and eligible to be the `live` target. A
//! run without `meta.json` is incomplete (mid-pass or crashed) and gets
//! swept by [`recover`] on the next indexer start.
//!
//! Concurrency: an exclusive `flock` on `<derived_root>/index.lock`
//! enforces the one-writer invariant. Readers do not lock; run
//! directories are immutable once published, and POSIX keeps the inode
//! alive for any reader holding an open handle through GC.
//!
//! Module map:
//! - `types` — `LifecycleState`, `IndexingHandle`, the error enums,
//!   `StartupDecision`, `RecoveryReport`, and the lifecycle file
//!   constants (`META_FILE`, `ACCESS_MARKER`).
//! - `indexing` — `begin_indexing` and the `IndexingHandle`
//!   `publish`/`abort`/`Drop` impls.
//! - `state` — read-only views over the on-disk state plus the access
//!   plumbing.
//! - `atomic` — `atomic_flip_live` + `fsync_dir`.
//! - `recover` — incomplete-run sweep + cross-fs / legacy-dir cleanup.
//! - `ops` — operator-driven `rollback` and `gc`.

mod atomic;
mod indexing;
mod ops;
mod recover;
mod state;
mod types;

#[cfg(test)]
mod tests;

pub use indexing::begin_indexing;
pub use ops::{gc, rollback};
pub use recover::recover;
pub use state::{current_state, decide_startup_state, list_completed_runs};
pub use types::{
    BeginError, IndexingHandle, LifecycleState, PublishError, RecoveryError, RecoveryReport,
    RollbackError, StartupDecision,
};
