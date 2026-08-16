//! MCP server lifecycle — three states the server moves through across
//! its lifetime, observed via `get_index_status` and gated against by
//! every other tool.
//!
//! - [`LifecycleState::Indexing`] — startup or initial bootstrap. Tools
//!   that need a populated index return `INDEX_UNAVAILABLE`. Status tool
//!   returns the current progress snapshot.
//! - [`LifecycleState::Ready`] — snapshot is open and tools can serve.
//!   `Ready` is **non-terminal**: a background `reindex` may run while
//!   the server keeps serving the current snapshot, and snapshot
//!   hot-reload may swap the reader to a newer snapshot without leaving
//!   `Ready`. A background-reindex failure stays in `Ready` on the
//!   pre-reindex snapshot.
//! - [`LifecycleState::Failed`] — only cold-start pipeline errors land
//!   here; terminal until the process restarts or a `reindex` tool
//!   call retries it.

use std::sync::{Arc, RwLock};

use kenn_indexer::pipeline::ProgressEvent;

use kenn_query::{EmbedStage, SnapshotId};

/// Per-server lifecycle state machine. See module docs.
pub enum LifecycleState {
    /// Indexing in progress. `progress` may be `None` until the first
    /// progress event arrives.
    Indexing {
        started_at: std::time::Instant,
        progress: Option<ProgressSnapshot>,
    },
    /// Snapshot open and ready to serve. `read` is an [`ArcSwap`] over a
    /// [`ReaderBinding`] — an `Arc` that bundles the `DbReader` with the
    /// cross-process GC pin (`kenn_store::readers::ReaderMarker`) for
    /// that snapshot. Tool dispatch calls `read.load_full()` (lock-free)
    /// to get an `Arc<ReaderBinding>` it owns for the call's duration;
    /// the marker stays alive as long as any in-flight call holds the
    /// binding, so GC never collects a snapshot out from under a
    /// running call. A snapshot swap calls `read.store(new_binding)`,
    /// which never blocks readers — in-flight calls finish against the
    /// snapshot they started on, later calls see the new one. The old
    /// binding's marker drops only once every in-flight call has
    /// finished. `reindex` is `Some` only while a background reindex is
    /// in flight; reads are never blocked during it.
    Ready {
        snapshot_path: std::path::PathBuf,
        snapshot_id: SnapshotId,
        indexed_at: String,
        read: arc_swap::ArcSwap<ReaderBinding>,
        fallback_from_parent: bool,
        reindex: Option<ReindexProgress>,
        /// The served snapshot's persisted run metadata, parsed once at
        /// bind time (`open_binding`). Drives the degraded-run fields of
        /// `get_index_status` without a call-path read. `None` for a
        /// pre-reporting or meta-less snapshot. Boxed to keep this
        /// (already-largest) variant from dwarfing the others.
        run_meta: Option<Box<kenn_indexer::SnapshotMeta>>,
    },
    /// Indexing failed; terminal until process restart. The MCP contract
    /// surfaces this via `INDEX_UNAVAILABLE` for non-status tools and
    /// `state: "failed"` from `get_index_status`.
    Failed {
        error: String,
        started_at: std::time::Instant,
        ended_at: std::time::Instant,
    },
}

impl LifecycleState {
    #[must_use]
    pub fn kind(&self) -> StateKind {
        match self {
            Self::Indexing { .. } => StateKind::Indexing,
            Self::Ready { .. } => StateKind::Ready,
            Self::Failed { .. } => StateKind::Failed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateKind {
    Indexing,
    Ready,
    Failed,
}

impl StateKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Indexing => "indexing",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

/// A `DbReader` bundled with its cross-process GC pin. While any holder
/// keeps the `Arc<ReaderBinding>` alive, the pin's `flock` keeps the
/// snapshot it points at from being collected by [`kenn_store::lifecycle::gc`].
/// `Deref<Target = DbReader>` lets call sites use it as a `DbReader`
/// transparently.
pub struct ReaderBinding {
    pub reader: kenn_store::DbReader,
    /// Held for its drop side-effect (releases the snapshot pin).
    _pin: kenn_store::readers::ReaderMarker,
}

impl ReaderBinding {
    #[must_use]
    pub fn new(reader: kenn_store::DbReader, pin: kenn_store::readers::ReaderMarker) -> Self {
        Self { reader, _pin: pin }
    }
}

impl std::ops::Deref for ReaderBinding {
    type Target = kenn_store::DbReader;
    fn deref(&self) -> &Self::Target {
        &self.reader
    }
}

/// In-flight background reindex carried under [`LifecycleState::Ready`]
/// while one is running. Mirrors the `Indexing` variant's
/// `started_at` + `progress` pair so the status tool can render the
/// same shape whether reindex is cold-start (`Indexing`) or background
/// (`Ready.reindex`).
#[derive(Debug, Clone)]
pub struct ReindexProgress {
    pub started_at: std::time::Instant,
    pub progress: Option<ProgressSnapshot>,
}

/// Snapshot of the most-recent pipeline progress. Updated on each
/// `ProgressEvent`; read-only by the status tool.
#[derive(Debug, Clone, Default)]
pub struct ProgressSnapshot {
    pub phase: &'static str,
    pub files_seen: u64,
    pub symbols_seen: u64,
    pub edges_seen: u64,
}

impl ProgressSnapshot {
    /// Fold a `ProgressEvent` into this snapshot. Counters accumulate
    /// (sum over per-unit events); phase tracks the latest milestone.
    pub fn observe(&mut self, e: &ProgressEvent) {
        match e {
            ProgressEvent::Started => self.phase = "started",
            ProgressEvent::UnitIngested {
                files,
                symbols,
                edges,
                ..
            } => {
                self.phase = "ingest";
                self.files_seen += files;
                self.symbols_seen += symbols;
                self.edges_seen += edges;
            }
            ProgressEvent::StubsFlushed { .. } => self.phase = "flush_stubs",
            ProgressEvent::AggregateComputed { .. } => self.phase = "aggregate",
            ProgressEvent::EndRunComplete { .. } => self.phase = "end_run",
            ProgressEvent::Completed { .. } => self.phase = "completed",
        }
    }
}

/// Shared lifecycle handle. `Arc<RwLock<...>>` because tool dispatch is
/// read-heavy (every call grabs a read lock) and the state transitions
/// happen on a single background task.
pub type SharedLifecycle = Arc<RwLock<LifecycleState>>;

/// Reported by `get_index_status.watcher` and updated by the watcher
/// task as events flow.
///
/// `Off` — no watcher is running. `Idle` — running, no pending debounce
/// deadline. `Debouncing` — running, a deadline is pending (event(s)
/// have landed inside the window). See `mcp-orchestrated-indexing`
/// spec and design.md §D6.
///
/// Serialized as `snake_case` strings on the wire.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum WatcherState {
    Off = 0,
    Idle = 1,
    Debouncing = 2,
}

/// Atomic cell holding a [`WatcherState`]. Used by `ServerState` so
/// any thread (debounce task, `get_index_status` tool path, `watch_stop`
/// tool) can read/write the watcher's state without a lock.
pub struct AtomicWatcherState(std::sync::atomic::AtomicU8);

impl AtomicWatcherState {
    #[must_use]
    pub const fn new(initial: WatcherState) -> Self {
        Self(std::sync::atomic::AtomicU8::new(initial as u8))
    }

    pub fn store(&self, v: WatcherState) {
        self.0.store(v as u8, std::sync::atomic::Ordering::Relaxed);
    }

    #[must_use]
    #[expect(
        clippy::match_same_arms,
        reason = "the `0` arm and the `_` arm both decode to Off but are conceptually distinct: 0 is the documented encoding for Off, the wildcard is a safety fallback for an out-of-range value that only `store(WatcherState)` could produce — and never does"
    )]
    pub fn load(&self) -> WatcherState {
        match self.0.load(std::sync::atomic::Ordering::Relaxed) {
            0 => WatcherState::Off,
            1 => WatcherState::Idle,
            2 => WatcherState::Debouncing,
            _ => WatcherState::Off,
        }
    }
}

/// Atomic cell holding an [`EmbedStage`], mirroring [`AtomicWatcherState`].
/// Shared (via `Arc`) into the background embed-job task, which stores the
/// stage transitions; read on the `get_index_status` and `find_similar` paths.
pub struct AtomicEmbedStage(std::sync::atomic::AtomicU8);

impl AtomicEmbedStage {
    #[must_use]
    pub const fn new(initial: EmbedStage) -> Self {
        Self(std::sync::atomic::AtomicU8::new(initial as u8))
    }

    pub fn store(&self, v: EmbedStage) {
        self.0.store(v as u8, std::sync::atomic::Ordering::Relaxed);
    }

    #[must_use]
    #[expect(
        clippy::match_same_arms,
        reason = "the `1` arm and the `_` fallback both decode to Ready but are conceptually distinct: 1 is the documented encoding, the wildcard is an unreachable safety fallback for an out-of-range byte"
    )]
    pub fn load(&self) -> EmbedStage {
        match self.0.load(std::sync::atomic::Ordering::Relaxed) {
            0 => EmbedStage::Building,
            1 => EmbedStage::Ready,
            2 => EmbedStage::Disabled,
            3 => EmbedStage::Degraded,
            _ => EmbedStage::Ready,
        }
    }
}

/// `tracing` target for all workspace-discovery log lines — the
/// initial bind in `kenn-cli/main.rs` and every post-handshake
/// rebind in `kenn-mcp/indexing.rs::rebind_workspace`. Grep this
/// target to see the full bind lifecycle of a kenn-mcp process.
pub const WORKSPACE_DISCOVERY_TARGET: &str = "kenn-mcp::workspace-discovery";

/// Which source produced the currently-bound workspace. Used by the
/// startup-log line and by the post-handshake rebind path
/// (mcp-roots-discovery §5/§7) to decide whether the binding is
/// permanent (only `CliFlag`) or tentative and overridable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSource {
    /// `--workspace <path>` CLI flag. Permanent for this server's
    /// lifetime — blocks post-handshake `roots/list` rebinds and
    /// `listChanged` notifications.
    CliFlag,
    /// `CLAUDE_PROJECT_DIR` env var (set by Claude Code at spawn).
    /// Tentative — a post-handshake `roots/list` returning a
    /// different path overrides this.
    ClaudeProjectDir,
    /// `roots/list` response from a post-handshake call against a
    /// `roots`-capable client. Tentative — `listChanged` can swap
    /// to a different root later.
    RootsList,
    /// `git rev-parse --show-toplevel` from the launching cwd.
    /// Tentative; serves manual `kenn mcp` debug invocations and
    /// hosts that don't set `CLAUDE_PROJECT_DIR` or expose roots.
    GitToplevel,
    /// Launching process's cwd. Last-resort fallback when nothing
    /// else resolves. Tentative.
    Cwd,
}

impl WorkspaceSource {
    /// Stable string for log output. Lowercase + hyphen-separated to
    /// match the names in `mcp-roots-discovery` design doc D9.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CliFlag => "cli-flag",
            Self::ClaudeProjectDir => "claude-project-dir",
            Self::RootsList => "roots-list",
            Self::GitToplevel => "git-toplevel",
            Self::Cwd => "cwd",
        }
    }

    /// True when this source is permanent — `--workspace` is an
    /// explicit operator directive that wins over any post-handshake
    /// host signal. Everything else is tentative and can be rebound.
    #[must_use]
    pub const fn is_permanent(self) -> bool {
        matches!(self, Self::CliFlag)
    }
}

#[cfg(test)]
mod tests {
    use super::{ProgressSnapshot, WatcherState};
    use kenn_indexer::pipeline::ProgressEvent;
    use kenn_model::Language;

    /// `ProgressSnapshot::observe` is the fold that turns a stream of
    /// `ProgressEvent`s into the snapshot the status tool reports.
    /// Cover every match arm: each variant updates `phase`, and
    /// `UnitIngested` is the only variant that accumulates counters.
    #[test]
    fn progress_snapshot_observe_covers_every_event() {
        let mut s = ProgressSnapshot::default();
        s.observe(&ProgressEvent::Started);
        assert_eq!(s.phase, "started");

        s.observe(&ProgressEvent::UnitIngested {
            unit: kenn_indexer::pipeline::IngestUnit::JsonlWorkspace,
            language: Language::Rust,
            files: 3,
            symbols: 50,
            edges: 100,
        });
        assert_eq!(s.phase, "ingest");
        assert_eq!(s.files_seen, 3);
        assert_eq!(s.symbols_seen, 50);
        assert_eq!(s.edges_seen, 100);

        // Counters accumulate across UnitIngested events.
        s.observe(&ProgressEvent::UnitIngested {
            unit: kenn_indexer::pipeline::IngestUnit::JsonlWorkspace,
            language: Language::Csharp,
            files: 2,
            symbols: 30,
            edges: 70,
        });
        assert_eq!(s.files_seen, 5);
        assert_eq!(s.symbols_seen, 80);
        assert_eq!(s.edges_seen, 170);

        s.observe(&ProgressEvent::StubsFlushed { count: 4 });
        assert_eq!(s.phase, "flush_stubs");

        s.observe(&ProgressEvent::AggregateComputed {
            nodes: 100,
            edges: 200,
            elapsed_ms: 50,
        });
        assert_eq!(s.phase, "aggregate");

        s.observe(&ProgressEvent::EndRunComplete { elapsed_ms: 99 });
        assert_eq!(s.phase, "end_run");

        s.observe(&ProgressEvent::Completed { total_ms: 1234 });
        assert_eq!(s.phase, "completed");
    }

    /// `AtomicEmbedStage` round-trips every `EmbedStage` through its u8 encoding.
    #[test]
    fn atomic_embed_stage_round_trips() {
        use super::{AtomicEmbedStage, EmbedStage};
        for stage in [
            EmbedStage::Building,
            EmbedStage::Ready,
            EmbedStage::Disabled,
            EmbedStage::Degraded,
        ] {
            let cell = AtomicEmbedStage::new(stage);
            assert_eq!(cell.load(), stage);
        }
        // store overwrites
        let cell = AtomicEmbedStage::new(EmbedStage::Ready);
        cell.store(EmbedStage::Building);
        assert_eq!(cell.load(), EmbedStage::Building);
    }

    /// `WatcherState` serializes to the documented `snake_case` strings.
    #[test]
    fn watcher_state_serializes_snake_case() {
        for (state, expected) in [
            (WatcherState::Off, r#""off""#),
            (WatcherState::Idle, r#""idle""#),
            (WatcherState::Debouncing, r#""debouncing""#),
        ] {
            let s = serde_json::to_string(&state).expect("serialize");
            assert_eq!(s, expected);
        }
    }
}
