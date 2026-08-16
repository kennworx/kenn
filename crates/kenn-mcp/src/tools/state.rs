//! Per-server lifecycle state: `ServerState`, the `ReadyView` it pins for the
//! duration of a query, and the watcher-start result type.
//!
//! This is the host side of the split with `kenn-query`: everything here is a
//! fact about a *running server* — which lifecycle phase it is in, whether a
//! watcher is attached, which MCP peer it answers. A query never sees any of
//! it; [`ServerState::query_ctx`] hands out a
//! [`QueryCtx`](kenn_query::QueryCtx) carrying only what a read needs.

use std::sync::{Arc, RwLock};

use kenn_store::api::Reader;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use kenn_query::{
    internal, ConfigHint, QueryCaches, QueryCtx, QueryError, QueryErrorCode, SnapshotId,
};

use crate::state::{LifecycleState, SharedLifecycle};

/// Per-server state.
pub struct ServerState {
    /// The resolved store layout — every store path (committed sidecars,
    /// derived snapshots, the findings store) is derived from it.
    ///
    /// Wrapped in `ArcSwap` so the post-handshake roots-rebind path
    /// (mcp-roots-discovery change, §5/§7) can atomically swap in a
    /// new layout under live readers. Accessed via [`Self::layout`]
    /// (clones a snapshot) and mutated via [`Self::set_layout`].
    /// Private so future rebind machinery doesn't have to scan call
    /// sites — every reader funnels through the getter.
    layout: arc_swap::ArcSwap<kenn_store::Layout>,
    /// The loaded `kenn.toml` (or defaults). Needed by the `reindex`
    /// tool, which spawns the indexing pipeline directly.
    pub config: kenn_config::Config,
    /// Embedding model id stamped into the sidecar manifest, set from
    /// the host's one-time `GlobalConfig::load` at startup. Read by
    /// background embed-job spawners; no site re-reads config.
    pub model_id: String,
    pub lifecycle: SharedLifecycle,
    /// The durable findings store. Opened lazily by [`ServerState::bootstrap`]
    /// — `None` until then, and stays `None` if the open fails. Held
    /// here (not re-opened per call) so the pending buffer survives
    /// across tool invocations and the Lance dataset is opened once.
    pub(crate) findings: tokio::sync::RwLock<Option<kenn_store::FindingsStore>>,
    /// Monotonic per-process event counter, incremented by the file
    /// watcher on each surviving source event and by the staleness
    /// seed/backstop when a key-compare reports stale (a "synthetic
    /// event"). Paired with [`Self::run_event_seq`] to derive `is_stale`
    /// without any git work on the read path (watcher-driven-staleness
    /// D4). `Relaxed` is sufficient — the two counters are only ever
    /// compared for `>`; there is no ordering dependency on other state.
    #[doc(hidden)]
    pub last_event_seq: std::sync::atomic::AtomicU64,
    /// The served run's event-sequence stamp — **in-memory only**, never
    /// persisted. Set on every reader swap: a self-publish reindex
    /// installs the `last_event_seq` it captured at the reindex's start;
    /// a cross-instance reload (and the initial open) installs the
    /// current `last_event_seq` ("caught up as of now"). `is_stale` is
    /// `last_event_seq > run_event_seq` (D4).
    #[doc(hidden)]
    pub run_event_seq: std::sync::atomic::AtomicU64,
    /// The MCP peer for this server, populated once `start_background_indexing`
    /// receives it from the rmcp service layer. Code paths that need to
    /// emit server-initiated notifications (snapshot-swap from the poll
    /// task; future file-watcher events) read it from here. `None` in
    /// tests that construct `ServerState` directly without a peer.
    ///
    /// `OnceLock` rather than `ArcSwap`: the rmcp service serves one
    /// stdio peer per process lifetime, so set-once matches reality.
    /// If the model ever grows to support peer reconnect or multiple
    /// concurrent peers, this becomes a swap cell.
    pub peer: std::sync::OnceLock<rmcp::service::Peer<rmcp::RoleServer>>,
    /// In-process file watcher handle. `Some` while a watcher is
    /// running; `None` otherwise. `Mutex` (not `OnceLock`) because the
    /// agent may stop and restart the watcher many times across the
    /// server's lifetime.
    pub watcher: std::sync::Mutex<Option<crate::watcher::WatcherHandle>>,
    /// Atomic mirror of the watcher's current phase — `Off`, `Idle`,
    /// or `Debouncing`. Updated by the watcher's debounce task as
    /// events flow; read by `get_index_status` and the `watch_*` tools.
    pub watcher_state: crate::state::AtomicWatcherState,
    /// Stage of the background embedding pass, shared into the embed-job task
    /// (which records Building → Ready/Disabled) and read by `get_index_status`
    /// to report `embedding`/`ready`/`disabled`, and by `find_similar` to make a
    /// missing vector transient (still building) vs terminal. Default `Ready`:
    /// the steady state and the test/bootstrap path embed nothing.
    pub embed_stage: Arc<crate::state::AtomicEmbedStage>,
    /// The backend error from the last failed embed pass, set alongside
    /// `embed_stage = Degraded` by the embed-job task and read by
    /// `get_index_status` to carry the cause. `None` when healthy, disabled, or
    /// building. `Arc` so it shares into the spawned job like `embed_stage`.
    pub embed_error: Arc<arc_swap::ArcSwapOption<String>>,
    /// Which source produced the currently-bound workspace —
    /// `CliFlag`, `ClaudeProjectDir`, `RootsList`, `GitToplevel`, or
    /// `Cwd`. Read by the startup-log emitter and by the
    /// post-handshake rebind logic (mcp-roots-discovery §5/§7). Lock
    /// contention is non-issue: writes only on rebind (rare),
    /// reads on rebind-decisions and log emission (also rare).
    workspace_source: std::sync::Mutex<crate::state::WorkspaceSource>,
    /// Whether the connected client declared the `roots` capability
    /// during `initialize`. Set by `ServerHandler::initialize`; read
    /// by `on_initialized` (mcp-roots-discovery §5) to decide
    /// whether to issue `roots/list`. `Relaxed` ordering — the
    /// initialize handler runs before any rebind logic can fire.
    pub client_supports_roots: std::sync::atomic::AtomicBool,
    /// Whether the client declared `roots.listChanged: true`. Set
    /// by `ServerHandler::initialize`; read by
    /// `on_roots_list_changed` (mcp-roots-discovery §7) to decide
    /// whether to subscribe.
    pub client_supports_roots_list_changed: std::sync::atomic::AtomicBool,
    /// Serializes the post-handshake rebind path. `on_initialized`
    /// and `on_roots_list_changed` both spawn
    /// `resolve_roots_and_maybe_rebind` on the tokio runtime; without
    /// this, two notifications arriving close together (or
    /// `on_initialized` + a quick `listChanged`) would interleave
    /// the `set_failed` / `set_layout` / `spawn_recovery_pipeline`
    /// steps and leave inconsistent intermediate state. The lock is
    /// awaitable because the rebind body crosses `.await` points.
    pub rebind_lock: tokio::sync::Mutex<()>,
    /// Count of times the watcher's debounce window elapsed and
    /// triggered `spawn_background_reindex`. Bumped before the spawn
    /// so tests can assert trigger semantics without waiting on the
    /// real indexing pipeline (which the test workspaces don't have).
    ///
    /// Test-only observation point; not read by production code.
    #[doc(hidden)]
    pub watcher_triggers: std::sync::atomic::AtomicU64,
    /// The top-K page caches the query layer reads through a
    /// [`QueryCtx`]. Bounded LRU; cleared on snapshot rotation
    /// (`spawn_snapshot_swap_task` in `indexing.rs`). Owned here because they
    /// outlive any one query — a cursor issued by one call is redeemed by the
    /// next. See design D12.
    pub(crate) caches: QueryCaches,
}

/// Result of the `watch_start` tool. See design.md §D6.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WatchStartResult {
    /// True if this call started a new watcher; false if a watcher was
    /// already running and this call was a no-op.
    pub started: bool,
    /// Debounce window in milliseconds (from `mcp.watch_debounce_ms`).
    pub debounce_ms: u64,
}

/// Internal accessor: snapshot fields + a [`kenn_store::DbConn`] handle to the
/// snapshot's connection pool. The handle is a cheap clone (no I/O on the hot
/// path); each reader call dispatches its query onto a pooled background-thread
/// connection, so blocking `SQLite` never runs on a runtime worker and concurrent
/// tool reads parallelize. The `_pin` keeps the snapshot's GC pin alive for the
/// call's duration — held only for its drop side-effect.
pub struct ReadyView {
    pub snapshot_id: SnapshotId,
    pub indexed_at: String,
    /// One connection for this whole query.
    pub read: kenn_store::DbConn,
    /// Keeps the snapshot pinned for the call (drop side-effect only).
    _pin: std::sync::Arc<crate::state::ReaderBinding>,
}

impl ServerState {
    /// Construct a `ServerState` in the `Indexing` initial state with the
    /// in-repo default layout for `workspace` and a default
    /// [`kenn_config::Config`]. Convenience for tests and tools that do
    /// not care about config-driven flags.
    #[must_use]
    pub fn new(workspace: &std::path::Path) -> Self {
        Self::with_layout(kenn_store::Layout::default_for(workspace))
    }

    /// Construct a `ServerState` over a resolved [`kenn_store::Layout`]
    /// with a default config.
    #[must_use]
    pub fn with_layout(layout: kenn_store::Layout) -> Self {
        Self::with_layout_and_config(layout, kenn_config::Config::default())
    }

    /// Construct a `ServerState` over a resolved [`kenn_store::Layout`]
    /// and an explicit [`kenn_config::Config`]. Uses the default model id
    /// — for production paths use [`Self::with_layout_config_and_model`].
    #[must_use]
    pub fn with_layout_and_config(layout: kenn_store::Layout, config: kenn_config::Config) -> Self {
        Self::with_layout_config_and_model(
            layout,
            config,
            kenn_config::EmbeddingsConfig::default().model,
        )
    }

    /// Production constructor: layout + workspace config + the host-loaded
    /// embedding model id. The MCP server entry point uses this so the
    /// `reindex` tool can drive the indexing pipeline against the user's
    /// actual configuration and the background embed jobs stamp the same
    /// model id every site sees.
    #[must_use]
    pub fn with_layout_config_and_model(
        layout: kenn_store::Layout,
        config: kenn_config::Config,
        model_id: String,
    ) -> Self {
        Self {
            layout: arc_swap::ArcSwap::from_pointee(layout),
            config,
            model_id,
            lifecycle: Arc::new(RwLock::new(LifecycleState::Indexing {
                started_at: std::time::Instant::now(),
                progress: None,
            })),
            findings: tokio::sync::RwLock::new(None),
            last_event_seq: std::sync::atomic::AtomicU64::new(0),
            run_event_seq: std::sync::atomic::AtomicU64::new(0),
            peer: std::sync::OnceLock::new(),
            watcher: std::sync::Mutex::new(None),
            watcher_state: crate::state::AtomicWatcherState::new(crate::state::WatcherState::Off),
            embed_stage: Arc::new(crate::state::AtomicEmbedStage::new(
                kenn_query::EmbedStage::Ready,
            )),
            embed_error: Arc::new(arc_swap::ArcSwapOption::const_empty()),
            watcher_triggers: std::sync::atomic::AtomicU64::new(0),
            // Default `Cwd` covers tests and the manual `kenn mcp`
            // shell path. The real value is set by `with_workspace_source`
            // from `serve_stdio`'s caller (kenn-cli/main.rs).
            workspace_source: std::sync::Mutex::new(crate::state::WorkspaceSource::Cwd),
            // Capability flags are populated by `ServerHandler::initialize`.
            // Tests that construct `ServerState` directly without an
            // MCP handshake see the defaults (both `false`).
            client_supports_roots: std::sync::atomic::AtomicBool::new(false),
            client_supports_roots_list_changed: std::sync::atomic::AtomicBool::new(false),
            rebind_lock: tokio::sync::Mutex::new(()),
            caches: QueryCaches::new(),
        }
    }

    /// Set the binding source for this state, returning the modified
    /// `Self`. Builder pattern so `serve_stdio`'s caller can do
    /// `ServerState::with_layout_and_config(...).with_workspace_source(src)`.
    #[must_use]
    pub fn with_workspace_source(mut self, source: crate::state::WorkspaceSource) -> Self {
        *self
            .workspace_source
            .get_mut()
            .expect("workspace_source lock poisoned") = source;
        self
    }

    /// The current binding source. Cheap — reads through a Mutex
    /// only briefly since `WorkspaceSource` is `Copy`.
    #[must_use]
    pub fn workspace_source(&self) -> crate::state::WorkspaceSource {
        *self
            .workspace_source
            .lock()
            .expect("workspace_source lock poisoned")
    }

    /// Update the binding source. Called by the post-handshake
    /// rebind path (mcp-roots-discovery §5/§7) when `roots/list`
    /// resolves to a different path than the tentative bind.
    pub fn set_workspace_source(&self, source: crate::state::WorkspaceSource) {
        *self
            .workspace_source
            .lock()
            .expect("workspace_source lock poisoned") = source;
    }

    /// Snapshot the currently-bound layout. Clones a `Layout` (three
    /// `PathBuf`s — cheap) from the atomically-swappable cell.
    /// Cheaper alternative when only a path is needed: read it off
    /// the loaded guard directly, e.g. `state.layout_guard().source_root()`.
    #[must_use]
    pub fn layout(&self) -> kenn_store::Layout {
        (**self.layout.load()).clone()
    }

    /// Borrowed access to the current layout, for callers that only
    /// read a field and don't need ownership. The returned guard
    /// holds an Arc clone; drop it before doing slow work.
    #[must_use]
    pub fn layout_guard(&self) -> arc_swap::Guard<std::sync::Arc<kenn_store::Layout>> {
        self.layout.load()
    }

    /// Atomically swap in a new layout. Used by the post-handshake
    /// roots-rebind path (mcp-roots-discovery §5/§7). Readers
    /// in-flight continue against the previous layout until they
    /// drop their `Arc`; new readers see the new layout immediately.
    pub fn set_layout(&self, new_layout: kenn_store::Layout) {
        self.layout.store(std::sync::Arc::new(new_layout));
    }

    /// The source root — where indexed code lives; `get_source` resolves
    /// file paths against it. Returns an owned `PathBuf` because the
    /// underlying layout may be swapped under us.
    #[must_use]
    pub fn source_root(&self) -> std::path::PathBuf {
        self.layout.load().source_root().to_path_buf()
    }

    /// Increment the event counter and return the new value. Called by
    /// the watcher on each surviving source event, and by the
    /// seed/backstop when a key-compare reports stale (a synthetic
    /// event). See [`Self::last_event_seq`].
    pub fn bump_event_seq(&self) -> u64 {
        self.last_event_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }

    /// Current event counter. Captured at a reindex's start (for the
    /// self-publish swap) and at a cross-instance reload.
    #[must_use]
    pub fn event_seq(&self) -> u64 {
        self.last_event_seq
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Install the served run's event-sequence stamp on a reader swap.
    /// See [`Self::run_event_seq`].
    pub fn set_run_event_seq(&self, seq: u64) {
        self.run_event_seq
            .store(seq, std::sync::atomic::Ordering::Relaxed);
    }

    /// `is_stale` — true when an event has been observed since the
    /// served run's stamp (`last_event_seq > run_event_seq`). Pure
    /// atomic loads; no git, no store open (D1/D4).
    #[must_use]
    pub fn is_stale(&self) -> bool {
        self.last_event_seq
            .load(std::sync::atomic::Ordering::Relaxed)
            > self
                .run_event_seq
                .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Async "open the live snapshot if it exists" helper, plus the
    /// findings store. Follows the `live` pointer directly — used by
    /// in-process tests. The MCP startup path does NOT use this; it
    /// resolves the snapshot by staleness key through
    /// `crate::indexing::run_startup_decision`.
    pub async fn bootstrap(&self) {
        let layout = self.layout();
        if let Some(ready) = open_ready_if_live(&layout).await {
            if let Ok(mut g) = self.lifecycle.write() {
                *g = ready;
            }
        }
        self.open_findings().await;
    }

    /// Open the durable findings store into [`ServerState::findings`].
    /// The findings store has its own lifecycle, independent of the
    /// per-index-run snapshot, so this is safe to call regardless of
    /// snapshot state. On failure it logs and leaves `findings` `None`.
    pub async fn open_findings(&self) {
        let layout = self.layout();
        match kenn_store::FindingsStore::open(&layout).await {
            Ok(store) => *self.findings.write().await = Some(store),
            Err(e) => tracing::warn!("findings store unavailable: {e}"),
        }
    }

    /// Drop every entry from both top-K result caches. Production-side
    /// callers go through the snapshot-rotation hook in
    /// [`crate::indexing`]; this entry point exists so integration tests
    /// can drive the rotation effect (cache miss → `STALE_CURSOR`)
    /// without standing up a full reindex.
    pub fn clear_result_caches(&self) {
        self.caches.clear();
    }

    /// Open a snapshot for querying: the lifecycle gate, then the
    /// empty-snapshot gate, then a pinned view the caller holds for the
    /// duration of the call.
    ///
    /// Replaces the old `with_db` closure with a plain value, so a
    /// query can be an ordinary `async fn` over a [`QueryCtx`] instead of a
    /// closure body. That is not only tidier: passing a *borrowed* context into
    /// a closure runs straight into the same HRTB limitation `cmd_query.rs`
    /// already documents for `Fn(&_, &_) -> Fut`.
    ///
    /// **Gate order is load-bearing.** `INDEX_UNAVAILABLE` is checked first and
    /// wins over `EMPTY_SNAPSHOT`: a caller who cannot be served at all must not
    /// first be told something about a snapshot it was never going to read.
    pub async fn open_query(&self) -> Result<ReadyView, QueryError> {
        let view = self.ready_view_or_err()?;
        let symbol_count = view.read.count_table("symbols").await.map_err(internal)?;
        if let Some(hint) = ConfigHint::classify(&self.config, symbol_count, self.config_present())
        {
            return Err(QueryError::empty_snapshot(&hint));
        }
        Ok(view)
    }

    /// [`open_query`](Self::open_query) without the empty-snapshot gate, for
    /// `get_workspace_overview` — which must always answer and carry the config
    /// hint in its response rather than erroring.
    pub fn open_query_allow_empty(&self) -> Result<ReadyView, QueryError> {
        self.ready_view_or_err()
    }

    /// Build the borrowed query context over an open view.
    pub fn query_ctx<'a>(&'a self, view: &'a ReadyView) -> QueryCtx<'a> {
        QueryCtx {
            read: &view.read,
            indexed_at: &view.indexed_at,
            snapshot_id: view.snapshot_id,
            source_root: self.source_root(),
            config: &self.config,
            config_present: self.config_present(),
            embed_stage: self.embed_stage.load(),
            findings: &self.findings,
            caches: &self.caches,
        }
    }

    /// Whether a `kenn.toml` exists at the current workspace root.
    /// Distinguishes a never-initialized project (no config → suggest
    /// `kenn init`) from one whose existing config has no language
    /// enabled. Reads the live layout so a post-handshake roots rebind
    /// is reflected.
    #[must_use]
    pub(crate) fn config_present(&self) -> bool {
        self.layout_guard().source_root().join("kenn.toml").exists()
    }

    /// The lifecycle gate: a pinned view of the live snapshot, or the reason
    /// there isn't one. The `RwLock` guard is held only for the synchronous
    /// match — the returned view carries its own GC pin.
    fn ready_view_or_err(&self) -> Result<ReadyView, QueryError> {
        let guard = self.lifecycle.read().map_err(|e| {
            QueryError::new(
                QueryErrorCode::InternalError,
                format!("lifecycle lock poisoned: {e}"),
            )
        })?;
        match &*guard {
            LifecycleState::Indexing { .. } => Err(QueryError::index_unavailable_indexing()),
            LifecycleState::Failed { error, .. } => {
                Err(QueryError::index_unavailable_failed(error))
            }
            LifecycleState::Ready {
                snapshot_id,
                indexed_at,
                read,
                ..
            } => {
                let pin = read.load_full();
                let conn = pin.reader.connect().map_err(internal)?;
                Ok(ReadyView {
                    snapshot_id: *snapshot_id,
                    indexed_at: indexed_at.clone(),
                    read: conn,
                    _pin: pin,
                })
            }
        }
    }
}

/// Best-effort: if the layout's `live` resolves to a snapshot
/// directory, open a `Reader` there (registering a cross-process GC
/// pin) and build a `Ready` lifecycle state. Delegates to the shared
/// `open_binding` so the reader-open / pin / meta-parse dance has one
/// implementation (a prior hand-rolled copy silently skipped the
/// run-metadata parse this path now inherits).
async fn open_ready_if_live(layout: &kenn_store::Layout) -> Option<LifecycleState> {
    let store = kenn_store::Store::open(layout.clone()).ok()?;
    let snapshot_path = store.live_target()?;
    let parts = crate::indexing::open_binding(&store, &snapshot_path)
        .await
        .ok()?;
    Some(crate::indexing::ready_from_parts(parts))
}
