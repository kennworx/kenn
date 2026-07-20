//! Cold-start + reindex lifecycle: the background-indexing entry point,
//! the startup skip/reindex decision, ready-binding construction, the
//! snapshot swap, and the rmcp progress→notification bridge.

use std::sync::Arc;

use kenn_indexer::pipeline::ProgressEvent;
use kenn_store::api::Reader;
use rmcp::model::{LoggingLevel, LoggingMessageNotificationParam};
use rmcp::service::Peer;
use rmcp::RoleServer;
use tokio::sync::mpsc;

use crate::cursor::snapshot_id_from_timestamp;
use crate::state::{LifecycleState, ProgressSnapshot, ReaderBinding};
use crate::tools::ServerState;

use super::{set_failed, start_staleness_backstop_task, startup_seed};

/// freshness driver), and the startup seed.
pub fn start_background_indexing(state: Arc<ServerState>, peer: Peer<RoleServer>) {
    // Store the peer so server-initiated notifications (snapshot-swap,
    // future watcher events) can reach the client from anywhere. `set`
    // is no-op on second call — only the first peer wins, which matches
    // the one-server-one-client model.
    drop(state.peer.set(peer.clone()));

    let (tx, rx) = mpsc::unbounded_channel::<ProgressEvent>();
    spawn_notification_pump(rx, peer);

    let state_for_startup = Arc::clone(&state);
    let layout_for_startup = state.layout();
    let config_for_startup = state.config.clone();
    tokio::spawn(async move {
        run_startup_decision(
            &state_for_startup,
            &layout_for_startup,
            &config_for_startup,
            &tx,
        )
        .await;
    });

    let state_for_watcher = Arc::clone(&state);
    let state_for_seed = Arc::clone(&state);
    let watch_on = state.config.mcp.watch_on;
    let git_aware_skip = state.config.staleness.git_aware_skip;
    let config_sig = state.config.indexing_signature();
    let backstop_secs = state.config.mcp.staleness_backstop_secs;
    // Moves `state` — the seed and watcher branches use their own clones.
    start_staleness_backstop_task(state, git_aware_skip, config_sig, backstop_secs);

    // Startup seed (D4): reconcile a change made while the server was
    // down. One background key-compare once `Ready`; on stale it
    // synthesizes an event (off the read path).
    tokio::spawn(async move {
        wait_for_ready(&state_for_seed).await;
        startup_seed(&state_for_seed);
    });

    // mcp.watch_on (default true): start the watcher implicitly — it is
    // the primary freshness driver. Same code path as the `watch_start`
    // tool. Wait until the server reaches `Ready` first — `watch_start`
    // errors before then. On notify-init failure log a warning and
    // proceed; the backstop keeps the snapshot fresh.
    if watch_on {
        tokio::spawn(async move {
            wait_for_ready(&state_for_watcher).await;
            if let Err(e) = autostart_watcher(&state_for_watcher) {
                tracing::warn!(
                    "kenn-mcp: mcp.watch_on=true but watcher failed to start: {e}; \
                     server continues on the backstop only (call watch_start to retry)"
                );
            }
        });
    }
}

/// Poll the lifecycle until it reaches `Ready` or `Failed`. Polls with
/// a short interval so the autostart kicks in promptly without binding
/// itself into the snapshot poll task. Bounded — the lifecycle is
/// guaranteed to leave `Indexing` eventually (Decision 5).
async fn wait_for_ready(state: &ServerState) {
    loop {
        let kind = state
            .lifecycle
            .read()
            .map_or(crate::state::StateKind::Indexing, |g| g.kind());
        match kind {
            crate::state::StateKind::Ready | crate::state::StateKind::Failed => return,
            crate::state::StateKind::Indexing => {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
    }
}

/// Start the watcher from the boot path (no MCP call available).
/// Skips silently if the server reached `Failed` instead of `Ready`,
/// and reuses the same `crate::watcher::start` as the `watch_start`
/// tool so the install path is one-source-of-truth.
///
/// `pub` so integration tests can drive the boot-time autostart path
/// without spinning up the full `start_background_indexing` orchestration
/// (which would require running the real indexer pipeline).
#[doc(hidden)]
pub fn autostart_watcher(state: &Arc<ServerState>) -> Result<(), notify::Error> {
    let kind = state
        .lifecycle
        .read()
        .map_or(crate::state::StateKind::Failed, |g| g.kind());
    if kind != crate::state::StateKind::Ready {
        tracing::info!(
            "kenn-mcp: mcp.watch_on=true but server reached `{}`; not starting watcher",
            kind.as_str()
        );
        return Ok(());
    }
    // Hold the watcher mutex across the start so an interleaving
    // `watch_start` tool call serializes; one wins, the other sees
    // `is_some()` and returns started=false. Mutex is held for the
    // few-ms cost of `notify::Watcher::new` + recursive watch attach.
    // A `PoisonError` here means a prior holder panicked — surface as
    // a generic notify error so the caller sees a single failure path.
    let mut g = state.watcher.lock().map_err(|e| {
        notify::Error::generic(&format!("watcher mutex poisoned during autostart: {e}"))
    })?;
    if g.is_some() {
        // A racing `watch_start` won; nothing to do.
        return Ok(());
    }
    let (_, handle) = crate::watcher::start(state)?;
    *g = Some(handle);
    Ok(())
}

fn spawn_notification_pump(mut rx: mpsc::UnboundedReceiver<ProgressEvent>, peer: Peer<RoleServer>) {
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let msg = format_progress(&ev);
            if let Err(err) = peer
                .notify_logging_message(LoggingMessageNotificationParam {
                    level: LoggingLevel::Info,
                    data: serde_json::json!({ "message": msg }),
                    logger: Some("kenn-mcp/indexing".into()),
                })
                .await
            {
                tracing::debug!("kenn-mcp: progress notification dropped, peer gone: {err}");
                break;
            }
        }
    });
}

/// Build the JSON `data` payload for the `code_updated` notification.
/// Pulled out as a pure function so tests can verify the wire shape
/// without a live MCP peer.
#[must_use]
pub fn code_updated_payload(indexed_at: &str) -> serde_json::Value {
    serde_json::json!({
        "event": "code_updated",
        "message": format!("Code updated at {indexed_at}"),
    })
}

/// Emit the code-update notification through the same logging-message
/// channel used for indexing progress. Schema:
/// `{ event: "code_updated", message }`. Best-effort: peer-gone is
/// debug-logged and the call returns. See `mcp-orchestrated-indexing`
/// Decision 7.
pub(crate) async fn emit_code_updated(peer: &Peer<RoleServer>, indexed_at: &str) {
    if let Err(err) = peer
        .notify_logging_message(LoggingMessageNotificationParam {
            level: LoggingLevel::Info,
            data: code_updated_payload(indexed_at),
            logger: Some("kenn-mcp/indexing".into()),
        })
        .await
    {
        tracing::debug!("kenn-mcp: code_updated notification dropped, peer gone: {err}");
    }
}

pub(crate) fn format_progress(ev: &ProgressEvent) -> String {
    match ev {
        ProgressEvent::Started => "indexing: started".into(),
        ProgressEvent::UnitIngested {
            language,
            files,
            symbols,
            ..
        } => format!(
            "indexing: {} unit ingested (files={files}, symbols={symbols})",
            language.db_name()
        ),
        ProgressEvent::StubsFlushed { count } => format!("indexing: flushed {count} stubs"),
        ProgressEvent::AggregateComputed {
            nodes,
            edges,
            elapsed_ms,
        } => format!("indexing: aggregated graph ({nodes} nodes, {edges} edges) in {elapsed_ms}ms"),
        ProgressEvent::EndRunComplete { elapsed_ms } => {
            format!("indexing: end_run complete in {elapsed_ms}ms")
        }
        ProgressEvent::Completed { total_ms } => {
            format!("indexing: complete in {total_ms}ms")
        }
    }
}

/// Decide Skip vs Reindex; run the chosen path; transition lifecycle.
pub(crate) async fn run_startup_decision(
    state: &ServerState,
    layout: &kenn_store::Layout,
    config: &kenn_config::Config,
    tx: &mpsc::UnboundedSender<ProgressEvent>,
) {
    // The durable findings store is lifecycle-independent — open it up
    // front, regardless of the snapshot decision.
    state.open_findings().await;

    let store = match kenn_store::Store::open(layout.clone()) {
        Ok(s) => s,
        Err(e) => {
            set_failed(state, format!("opening store: {e}"));
            return;
        }
    };

    // Resolve the snapshot by staleness key — not by following `live`.
    // A derived store shared across branches serves each from its own
    // matching snapshot, and a stale workspace re-indexes rather than
    // serving the wrong snapshot.
    let decision = kenn_store::decide_startup_state(
        &store,
        layout.source_root(),
        config.staleness.git_aware_skip,
        config.indexing_signature(),
    );
    match decision {
        kenn_store::StartupDecision::Skip { live } => {
            skip_or_reindex_on_empty(state, &store, layout, config, tx, &live).await;
        }
        kenn_store::StartupDecision::Reindex { reason: _ } => {
            run_reindex_and_install(state, &store, layout, config, tx).await;
        }
    }

    // Background embed job (incremental-embedding): once the structural
    // store is Ready, embed the symbols `kenn index` left null. BM25 and
    // any already-reconciled vectors serve throughout; vector coverage
    // fills in when the job republishes the store, which the path-based
    // reader observes with no reopen.
    let is_ready = matches!(
        *state.lifecycle.read().expect("lifecycle lock poisoned"),
        LifecycleState::Ready { .. }
    );
    if is_ready {
        spawn_embed_job(
            layout.clone(),
            config.staleness.git_aware_skip,
            config.indexing_signature(),
            state.model_id.clone(),
            state.embed_stage.clone(),
            state.embed_error.clone(),
        );
    }
}

/// Open a Reader against `snapshot_path` and install it as the new
/// `LifecycleState::Ready`, or transition to `Failed` on error.
/// Pulled out so both startup paths (skip + post-reindex) share the
/// same pin → open → install → set_failed-on-error sequence.
async fn install_ready_or_fail(
    state: &ServerState,
    store: &kenn_store::Store,
    snapshot_path: &std::path::Path,
) {
    match open_ready(store, snapshot_path).await {
        Ok(ready) => {
            {
                let mut g = state.lifecycle.write().expect("lifecycle lock poisoned");
                *g = ready;
            }
            // Initial open initializes `run_event_seq := last_event_seq`
            // (same as a cross-instance swap, D4): the just-opened run is
            // "current as of now". The startup seed then reconciles any
            // change made while the server was down.
            state.set_run_event_seq(state.event_seq());
        }
        Err(e) => set_failed(state, e),
    }
}

/// Build the progress callback closure that the indexer invokes per
/// `ProgressEvent`. Forwards each event into both the lifecycle
/// `Indexing.progress` snapshot AND the rmcp notification pump.
/// Extracted so `run_reindex_and_install` doesn't carry the closure
/// inline.
fn make_progress_callback(
    state: &ServerState,
    tx: &mpsc::UnboundedSender<ProgressEvent>,
) -> impl Fn(ProgressEvent) + Send + Sync + 'static {
    let lifecycle = state.lifecycle.clone();
    let tx_for_cb = tx.clone();
    move |ev: ProgressEvent| {
        if let Ok(mut g) = lifecycle.write() {
            if let LifecycleState::Indexing { progress, .. } = &mut *g {
                let snap = progress.get_or_insert_with(ProgressSnapshot::default);
                snap.observe(&ev);
            }
        }
        if tx_for_cb.send(ev).is_err() {
            // Notification pump has stopped; progress updates are
            // best-effort.
        }
    }
}

/// Drive the full indexer workflow, then install the resulting
/// snapshot as `Ready` (or transition to `Failed` on pipeline error).
async fn run_reindex_and_install(
    state: &ServerState,
    store: &kenn_store::Store,
    layout: &kenn_store::Layout,
    config: &kenn_config::Config,
    tx: &mpsc::UnboundedSender<ProgressEvent>,
) {
    let progress = make_progress_callback(state, tx);
    match kenn_indexer::index_workspace(
        layout,
        config,
        progress,
        kenn_analyze::analysis_hook_from_config(config),
    )
    .await
    {
        Ok(outcome) => install_ready_or_fail(state, store, &outcome.snapshot_path).await,
        // Store the raw pipeline error as the reason; the "indexing failed:"
        // framing is added once, by `McpError::index_unavailable_failed`
        // (and the `failed` lifecycle state in `get_index_status`).
        Err(e) => set_failed(state, e.to_string()),
    }
}

/// Spawn the incremental embed job once the structural store is Ready.
/// When no model is available `embed_pending` is a clean no-op, so this
/// is unconditionally safe to spawn. Every path comes from `layout`, and
/// the snapshot is resolved by staleness key — so the embed job targets
/// the same snapshot the server resolved, not whatever `live` points at.
fn spawn_embed_job(
    layout: kenn_store::Layout,
    git_aware_skip: bool,
    config_sig: u64,
    model_id: String,
    embed_stage: std::sync::Arc<crate::state::AtomicEmbedStage>,
    embed_error: std::sync::Arc<arc_swap::ArcSwapOption<String>>,
) {
    // Mark "building" synchronously so `get_index_status` reports `embedding`
    // the instant the graph is Ready, before the async task is even scheduled.
    embed_stage.store(crate::state::EmbedStage::Building);
    tokio::spawn(async move {
        let embedder = kenn_store::shared_embedder();
        match kenn_store::embed_pending(&layout, git_aware_skip, config_sig, &model_id, embedder)
            .await
        {
            Ok(report) => {
                // `Disabled` only when no embedder exists; otherwise `Ready`
                // (vectors filled, or nothing was pending). Either way the
                // embedder is healthy, so clear any prior degraded error.
                embed_stage.store(if report.embedder_available {
                    crate::state::EmbedStage::Ready
                } else {
                    crate::state::EmbedStage::Disabled
                });
                embed_error.store(None);
                if report.vectors > 0 {
                    tracing::info!(
                        "kenn-mcp: embed job filled {} vectors in {:.1}s",
                        report.vectors,
                        report.embed_seconds
                    );
                } else {
                    tracing::debug!("kenn-mcp: embed job — nothing pending");
                }
            }
            Err(e) => {
                // A model is configured but embedding failed (e.g. the macOS
                // fork+Metal bug). Report `Degraded` with the cause — NOT `Ready`
                // — so status surfaces the silent lexical-only fallback instead
                // of claiming vectors exist. `embed_pending` also persisted the
                // error for `kenn status` to read.
                let cause = e.to_string();
                embed_stage.store(crate::state::EmbedStage::Degraded);
                embed_error.store(Some(std::sync::Arc::new(cause.clone())));
                tracing::warn!("kenn-mcp: embed job degraded: {cause}");
            }
        }
    });
}

/// The components of a `LifecycleState::Ready` payload, opened against
/// `snapshot_path`. Returned by [`open_binding`] so [`open_ready`] and
/// [`poll_once`] can both build/swap a `Ready` without duplicating the
/// pin-then-open dance.
pub(crate) struct ReadyParts {
    snapshot_path: std::path::PathBuf,
    snapshot_id: crate::cursor::SnapshotId,
    indexed_at: String,
    binding: Arc<ReaderBinding>,
    /// The served snapshot's persisted run metadata, parsed once here at
    /// bind time so `get_index_status` can report a degraded run without a
    /// call-path read. `None` for a pre-reporting snapshot or one whose
    /// `meta.json` is missing/unparsable (e.g. a parent-worktree fallback).
    meta: Option<Box<kenn_indexer::SnapshotMeta>>,
}

/// Register a cross-process GC pin for `snapshot_path` and open a
/// `Reader` against it. The pin is acquired BEFORE the reader (Decision
/// 4) so a concurrent GC sweep on another instance never races with our
/// resolve-then-open. A pin failure surfaces as an error and the caller
/// re-resolves `live`.
pub(crate) async fn open_binding(
    store: &kenn_store::Store,
    snapshot_path: &std::path::Path,
) -> Result<ReadyParts, String> {
    let timestamp = snapshot_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let snapshot_id = snapshot_id_from_timestamp(&timestamp);
    let pin = kenn_store::readers::register_reader(store, snapshot_path)
        .map_err(|e| format!("registering reader pin at {}: {e}", snapshot_path.display()))?;
    let read = kenn_store::open_reader(snapshot_path).await.map_err(|e| {
        // Surface schema mismatch with the clean store-side message so
        // the agent sees the actionable form ("reindex required") — not a
        // path-prefixed wrapper. Other open errors keep the path prefix
        // for debuggability.
        match e {
            kenn_store::api::DbError::SchemaMismatch { .. } => e.to_string(),
            _ => format!("opening reader at {}: {e}", snapshot_path.display()),
        }
    })?;
    Ok(ReadyParts {
        snapshot_path: snapshot_path.to_path_buf(),
        snapshot_id,
        indexed_at: timestamp,
        binding: Arc::new(ReaderBinding::new(read, pin)),
        meta: read_snapshot_meta(snapshot_path),
    })
}

/// Parse the snapshot's `meta.json` via [`kenn_indexer::SnapshotMeta::read`]
/// (the shared read home — logs a parse error, distinguishes it from an
/// absent file). `None` for a pre-reporting or meta-less snapshot. A single
/// small synchronous read, done once at bind time (never on the status call
/// path).
fn read_snapshot_meta(snapshot_path: &std::path::Path) -> Option<Box<kenn_indexer::SnapshotMeta>> {
    kenn_indexer::SnapshotMeta::read(snapshot_path).map(Box::new)
}

/// Build a `LifecycleState::Ready` over `snapshot_path` — open the
/// reader, pin the snapshot against GC, and wrap the binding in the
/// swappable cell. See [`open_binding`].
async fn open_ready(
    store: &kenn_store::Store,
    snapshot_path: &std::path::Path,
) -> Result<LifecycleState, String> {
    Ok(ready_from_parts(open_binding(store, snapshot_path).await?))
}

/// Build a `Ready` lifecycle state from an already-open binding.
pub(crate) fn ready_from_parts(parts: ReadyParts) -> LifecycleState {
    LifecycleState::Ready {
        snapshot_path: parts.snapshot_path,
        snapshot_id: parts.snapshot_id,
        indexed_at: parts.indexed_at,
        read: arc_swap::ArcSwap::from(parts.binding),
        fallback_from_parent: false,
        reindex: None,
        run_meta: parts.meta,
    }
}

/// Cold-start `Skip` handler. Open the matched live snapshot, but do NOT
/// serve it as `Ready` if it is empty while the config expects symbols —
/// re-index instead (mcp-orchestrated-indexing: "Cold start does not serve
/// an empty snapshot for a configured workspace"). This recovers a prior
/// index run that produced zero symbols from a transient indexer failure
/// and published the empty result under the workspace's staleness key.
///
/// A workspace whose config does not expect symbols (no `kenn.toml` / all
/// languages disabled) settles `Ready` on the empty snapshot with the
/// existing config-hint and never triggers a reindex — so there is no
/// per-startup loop. The reindex, when taken, runs at most once (one
/// startup decision per process).
async fn skip_or_reindex_on_empty(
    state: &ServerState,
    store: &kenn_store::Store,
    layout: &kenn_store::Layout,
    config: &kenn_config::Config,
    tx: &mpsc::UnboundedSender<ProgressEvent>,
    live: &std::path::Path,
) {
    let parts = match open_binding(store, live).await {
        Ok(p) => p,
        Err(e) => {
            set_failed(state, e);
            return;
        }
    };
    // A failed count is `None` → treated as "serve it", so a transient
    // read error never provokes a spurious reindex.
    if should_reindex_empty_snapshot(
        count_symbols(&parts.binding).await,
        config_expects_symbols(config),
    ) {
        tracing::info!(
            target: "kenn_mcp::indexing",
            snapshot = %live.display(),
            "matched snapshot is empty but config enables a language; re-indexing instead of serving empty"
        );
        drop(parts); // release the transient reader pin before re-indexing
        run_reindex_and_install(state, store, layout, config, tx).await;
        return;
    }
    {
        let mut g = state.lifecycle.write().expect("lifecycle lock poisoned");
        *g = ready_from_parts(parts);
    }
    state.set_run_event_seq(state.event_seq());
}

/// Cold-start skip decision: re-index instead of serving a matched
/// snapshot when it has zero symbols (`Some(0)`) and the config expects
/// symbols. A `None` count (read failure) or any non-zero count serves
/// the snapshot as-is.
fn should_reindex_empty_snapshot(symbol_count: Option<u64>, expects_symbols: bool) -> bool {
    matches!(symbol_count, Some(0)) && expects_symbols
}

/// True when `kenn.toml` enables at least one language — i.e. an empty
/// snapshot is unexpected and worth re-indexing. Mirrors the language
/// flags `ConfigHint` reads.
fn config_expects_symbols(config: &kenn_config::Config) -> bool {
    config.language.csharp.enabled
        || config.language.rust.enabled
        || config.language.typescript.enabled
        || config.language.python.enabled
        || config.language.go.enabled
        || config.language.swift.enabled
}

/// Count rows in the `symbols` table for an open binding. `None` when the
/// connection or the count fails — callers treat that as "do not re-index".
async fn count_symbols(binding: &ReaderBinding) -> Option<u64> {
    binding
        .reader
        .connect()
        .ok()?
        .count_table("symbols")
        .await
        .ok()
}

/// Open + pin `new_path`, install it as the served snapshot under the
/// lifecycle write lock, stamp `run_event_seq`, clear the top-K caches,
/// emit `code_updated`, and trigger the embed job. Returns `true` if the
/// swap happened. All-or-nothing: a snapshot that fails to open is
/// logged and the current reader stays in service (task 3.4).
///
/// `run_seq` is the value to stamp `run_event_seq` with — the caller
/// chooses by provenance (D4): a self-publish reindex passes the counter
/// captured at its start; a cross-instance reload passes the current
/// `event_seq()`.
pub(crate) async fn swap_to_snapshot(
    state: &ServerState,
    store: &kenn_store::Store,
    new_path: &std::path::Path,
    run_seq: u64,
    git_aware_skip: bool,
    config_sig: u64,
) -> bool {
    let parts = match open_binding(store, new_path).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                "kenn-mcp: hot-reload of {} failed: {e}; keeping current snapshot",
                new_path.display()
            );
            return false;
        }
    };
    let new_binding = parts.binding;
    let new_path = parts.snapshot_path;
    let new_id = parts.snapshot_id;
    let new_at = parts.indexed_at;
    let new_meta = parts.meta;
    // Scope the write guard tightly so it's provably dropped before the
    // notification await (RwLockWriteGuard is not Send).
    {
        let mut g = state.lifecycle.write().expect("lifecycle lock poisoned");
        let LifecycleState::Ready {
            snapshot_path,
            snapshot_id,
            indexed_at,
            read,
            run_meta,
            ..
        } = &mut *g
        else {
            // Left Ready while we were opening (e.g. a reindex
            // transitioned us out). Drop the new binding.
            return false;
        };
        *snapshot_path = new_path;
        *snapshot_id = new_id;
        indexed_at.clone_from(&new_at);
        read.store(new_binding);
        *run_meta = new_meta;
    }
    // Stamp the served run's event-seq (D4), after the reader swap so
    // `is_stale` never briefly reads fresh against a stale reader.
    state.set_run_event_seq(run_seq);

    // Drop every cached top-K result set — they were materialized against
    // the prior snapshot; cursors now surface STALE_CURSOR (result_cache
    // D12).
    state.search_symbols_cache.clear();
    state.search_findings_cache.clear();

    // Notify the connected MCP client that the served snapshot changed —
    // converges every reindex source onto one "data is fresh" signal.
    // Best-effort: peer-gone is dropped.
    if let Some(peer) = state.peer.get() {
        emit_code_updated(peer, &new_at).await;
    }

    // Embed coordination lives inside `embed_pending` — every trigger
    // routes through the per-snapshot lock (Decision 6).
    spawn_embed_job(
        state.layout(),
        git_aware_skip,
        config_sig,
        state.model_id.clone(),
        state.embed_stage.clone(),
        state.embed_error.clone(),
    );
    true
}

#[cfg(test)]
mod tests {
    use super::{config_expects_symbols, should_reindex_empty_snapshot};

    #[test]
    fn reindex_only_on_empty_and_configured() {
        // Empty snapshot + the config expects symbols → re-index.
        assert!(should_reindex_empty_snapshot(Some(0), true));
        // Empty but config does not expect symbols → serve empty Ready
        // (no per-startup reindex loop).
        assert!(!should_reindex_empty_snapshot(Some(0), false));
        // Non-empty → serve regardless of config.
        assert!(!should_reindex_empty_snapshot(Some(42), true));
        // Count failed (None) → serve as-is; never a spurious reindex.
        assert!(!should_reindex_empty_snapshot(None, true));
    }

    #[test]
    fn config_expects_symbols_tracks_enabled_languages() {
        let mut cfg = kenn_config::Config::default();
        // Default config has every language disabled.
        assert!(!config_expects_symbols(&cfg));
        cfg.language.rust.enabled = true;
        assert!(config_expects_symbols(&cfg));
    }
}
