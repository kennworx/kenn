//! Index-lifecycle tools: status, reindex, and watcher start/stop.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{McpError, McpErrorCode};
use crate::state::LifecycleState;
use crate::types::{IndexStatus, IndexStatusProgress, SingleResponse};

use super::{ServerState, WatchStartResult};

// ── META ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default, Serialize, JsonSchema)]
pub struct GetIndexStatusArgs {}

pub fn get_index_status(
    state: &ServerState,
    _: GetIndexStatusArgs,
) -> Result<SingleResponse<IndexStatus>, McpError> {
    let guard = state.lifecycle.read().map_err(lifecycle_poisoned)?;
    // `is_stale` is an event-seq generation comparison — two atomic
    // loads, no git and no store open on the call path (D1/D4).
    let status = build_index_status(
        &guard,
        state.source_root().display().to_string(),
        state.is_stale(),
        state.watcher_state.load(),
        state.embed_stage.load(),
        state.embed_error.load_full().map(|a| (*a).clone()),
    );
    Ok(SingleResponse::found(status))
}

fn lifecycle_poisoned<E: std::fmt::Display>(e: E) -> McpError {
    McpError::new(
        McpErrorCode::InternalError,
        format!("lifecycle lock poisoned: {e}"),
    )
}

/// The `IndexStatus.state` string for a background embed stage.
fn embed_stage_str(stage: crate::state::EmbedStage) -> &'static str {
    match stage {
        crate::state::EmbedStage::Building => "embedding",
        crate::state::EmbedStage::Ready => "ready",
        crate::state::EmbedStage::Disabled => "disabled",
        crate::state::EmbedStage::Degraded => "degraded",
    }
}

/// Build the `IndexStatus` payload from a borrowed lifecycle state plus
/// the cheap server-derived inputs. Extracted so `get_index_status`
/// and `wait_for_index` emit an identical payload. `embed_error` carries the
/// backend cause when the embedder is `Degraded` (`None` otherwise).
fn build_index_status(
    guard: &LifecycleState,
    workspace_root: String,
    is_stale_cached: bool,
    watcher_state: crate::state::WatcherState,
    embed_stage: crate::state::EmbedStage,
    embed_error: Option<String>,
) -> IndexStatus {
    match guard {
        LifecycleState::Indexing { progress, .. } => IndexStatus {
            state: "indexing".into(),
            workspace_root,
            snapshot_id: None,
            indexed_at: None,
            is_stale: false,
            reindex_in_progress: false,
            fallback_from_parent_worktree: false,
            progress: progress.as_ref().map(|p| IndexStatusProgress {
                phase: p.phase.into(),
                files_seen: p.files_seen,
                symbols_seen: p.symbols_seen,
                edges_seen: p.edges_seen,
            }),
            error: None,
            watcher: watcher_state,
            run_status: None,
            failed_projects: Vec::new(),
            failed_count: 0,
            warnings: Vec::new(),
            warning_count: 0,
            regression_count: 0,
        },
        LifecycleState::Ready {
            snapshot_id,
            indexed_at,
            fallback_from_parent,
            reindex,
            run_meta,
            ..
        } => {
            let health = degraded_fields(run_meta.as_deref());
            IndexStatus {
                // The graph is queryable; the reported stage reflects the
                // background embed pass (structural tools serve in every
                // variant). A `degraded` stage carries the backend cause in
                // `error`.
                state: embed_stage_str(embed_stage).into(),
                workspace_root,
                snapshot_id: Some(snapshot_id.to_hex()),
                indexed_at: Some(indexed_at.clone()),
                is_stale: is_stale_cached,
                reindex_in_progress: reindex.is_some(),
                fallback_from_parent_worktree: *fallback_from_parent,
                progress: reindex.as_ref().and_then(|r| {
                    r.progress.as_ref().map(|p| IndexStatusProgress {
                        phase: p.phase.into(),
                        files_seen: p.files_seen,
                        symbols_seen: p.symbols_seen,
                        edges_seen: p.edges_seen,
                    })
                }),
                error: embed_error,
                watcher: watcher_state,
                run_status: health.run_status,
                failed_projects: health.failed_projects,
                failed_count: health.failed_count,
                warnings: health.warnings,
                warning_count: health.warning_count,
                regression_count: health.regression_count,
            }
        }
        LifecycleState::Failed { error, .. } => IndexStatus {
            state: "failed".into(),
            workspace_root,
            snapshot_id: None,
            indexed_at: None,
            is_stale: false,
            reindex_in_progress: false,
            fallback_from_parent_worktree: false,
            progress: None,
            error: Some(error.clone()),
            watcher: watcher_state,
            run_status: None,
            failed_projects: Vec::new(),
            failed_count: 0,
            warnings: Vec::new(),
            warning_count: 0,
            regression_count: 0,
        },
    }
}

/// The served snapshot's degraded-run fields (`mcp-index-status-degradation`),
/// derived from its persisted `SnapshotMeta` via that type's own accessors
/// (`failed_total` / `warning_total` / `regression_total` / `is_clean`), so
/// this surface and `kenn status` share one source of truth. `run_status` is
/// present exactly when the run was **not clean** — i.e. it produced at least
/// one failure, warning, or regression; its value is the aggregate status
/// string (`"partial"`, or `"success"` for a warnings/regressions-only run).
/// A clean run omits every field, leaving the happy-path payload unchanged.
///
/// The list fields carry the RAW bounded attributions (no `+N more` display
/// marker — that belongs to text rendering, not a machine-readable array);
/// the `*_count` fields carry the true totals including dropped overflow.
#[derive(Default)]
struct DegradedFields {
    run_status: Option<String>,
    failed_projects: Vec<String>,
    failed_count: u64,
    warnings: Vec<String>,
    warning_count: u64,
    regression_count: u64,
}

fn degraded_fields(meta: Option<&kenn_indexer::SnapshotMeta>) -> DegradedFields {
    let Some(m) = meta else {
        return DegradedFields::default();
    };
    DegradedFields {
        run_status: (!m.is_clean()).then(|| m.status.clone()),
        failed_projects: m.failed_projects.clone(),
        failed_count: m.failed_total(),
        warnings: m.warnings.clone(),
        warning_count: m.warning_total(),
        regression_count: m.regression_total(),
    }
}

// ── WAIT ─────────────────────────────────────────────────────────────────────

/// Default `wait_for_index` timeout, the hard cap a caller cannot exceed,
/// and the poll interval. See design D2/D3.
const WAIT_DEFAULT_MS: u64 = 30_000;
const WAIT_MAX_MS: u64 = 120_000;
const WAIT_POLL_MS: u64 = 250;

/// Resolve the effective wait budget: the default when unset, clamped to
/// the hard maximum so a caller cannot block indefinitely.
fn clamp_timeout_ms(requested: Option<u64>) -> u64 {
    requested.unwrap_or(WAIT_DEFAULT_MS).min(WAIT_MAX_MS)
}

#[derive(Debug, Deserialize, Default, Serialize, JsonSchema)]
pub struct WaitForIndexArgs {
    /// How long to block, in milliseconds, before returning with
    /// `timed_out: true`. Defaults to 30 000; clamped to 120 000.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// `wait_for_index` response: the same payload `get_index_status` returns,
/// flattened, plus whether the call returned on timeout vs. settle.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct WaitForIndexResponse {
    #[serde(flatten)]
    pub status: IndexStatus,
    /// `true` when the wait returned because `timeout_ms` elapsed while
    /// the index was still unsettled; `false` when it settled.
    pub timed_out: bool,
}

/// Block until the index is *settled* (`Ready` with no in-flight reindex,
/// or `Failed`) or the timeout elapses. The index is *unsettled* while
/// `Indexing`, or `Ready` with a background reindex running. Polls a short
/// interval; never holds the lifecycle lock across the sleep, so concurrent
/// dispatch is unaffected. See `mcp-server` spec: `wait_for_index`.
pub async fn wait_for_index(
    state: &ServerState,
    args: WaitForIndexArgs,
) -> Result<SingleResponse<WaitForIndexResponse>, McpError> {
    let timeout = std::time::Duration::from_millis(clamp_timeout_ms(args.timeout_ms));
    let deadline = std::time::Instant::now() + timeout;
    let workspace_root = state.source_root().display().to_string();
    loop {
        // Read the lifecycle, decide settled, and build the status — all
        // under a tightly-scoped guard dropped before the await below (the
        // std RwLock guard is not Send).
        let (status, settled) = {
            let guard = state.lifecycle.read().map_err(lifecycle_poisoned)?;
            let settled = match &*guard {
                LifecycleState::Ready { reindex, .. } => reindex.is_none(),
                LifecycleState::Failed { .. } => true,
                LifecycleState::Indexing { .. } => false,
            };
            let status = build_index_status(
                &guard,
                workspace_root.clone(),
                state.is_stale(),
                state.watcher_state.load(),
                state.embed_stage.load(),
                state.embed_error.load_full().map(|a| (*a).clone()),
            );
            (status, settled)
        };
        if settled {
            return Ok(SingleResponse::found(WaitForIndexResponse {
                status,
                timed_out: false,
            }));
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return Ok(SingleResponse::found(WaitForIndexResponse {
                status,
                timed_out: true,
            }));
        }
        let remaining = deadline - now;
        tokio::time::sleep(std::time::Duration::from_millis(WAIT_POLL_MS).min(remaining)).await;
    }
}

// ── REINDEX ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default, Serialize, JsonSchema)]
pub struct ReindexArgs {}

/// Outcome of a `reindex` call.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReindexResponse {
    /// `"started" | "recovery_started" | "in_progress"`.
    pub status: String,
    /// Human-readable detail (which path triggered, why it coalesced, …).
    pub message: String,
}

/// The `reindex` tool. Triggers an in-process reindex; the call returns
/// promptly without waiting for the pipeline to finish. See
/// `mcp-orchestrated-indexing` requirement "Background reindex tool".
pub fn reindex(
    state: &Arc<ServerState>,
    _: ReindexArgs,
) -> Result<SingleResponse<ReindexResponse>, McpError> {
    let mut guard = state.lifecycle.write().map_err(|e| {
        McpError::new(
            McpErrorCode::InternalError,
            format!("lifecycle lock poisoned: {e}"),
        )
    })?;
    match &mut *guard {
        LifecycleState::Indexing { .. } => Ok(SingleResponse::found(ReindexResponse {
            status: "in_progress".into(),
            message: "cold-start indexing is already running; no second run started".into(),
        })),
        LifecycleState::Failed { .. } => {
            // Recovery: transition Failed → Indexing and retry the
            // pipeline as at cold start. Non-status tools return
            // INDEX_UNAVAILABLE until it reaches Ready.
            *guard = LifecycleState::Indexing {
                started_at: std::time::Instant::now(),
                progress: None,
            };
            drop(guard);
            crate::indexing::spawn_recovery_pipeline(Arc::clone(state));
            Ok(SingleResponse::found(ReindexResponse {
                status: "recovery_started".into(),
                message: "transitioning from Failed → Indexing and retrying the pipeline".into(),
            }))
        }
        LifecycleState::Ready { reindex, .. } => {
            if reindex.is_some() {
                return Ok(SingleResponse::found(ReindexResponse {
                    status: "in_progress".into(),
                    message: "a background reindex is already running on this instance".into(),
                }));
            }
            *reindex = Some(crate::state::ReindexProgress {
                started_at: std::time::Instant::now(),
                progress: None,
            });
            drop(guard);
            crate::indexing::spawn_background_reindex(Arc::clone(state));
            Ok(SingleResponse::found(ReindexResponse {
                status: "started".into(),
                message: "background reindex started; reads continue against the current snapshot until the new one publishes".into(),
            }))
        }
    }
}

// ── WATCH ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default, Serialize, JsonSchema)]
pub struct WatchStartArgs {}

/// The `watch_start` tool. Idempotent: starts the in-process file
/// watcher when none is running; returns the existing watcher's
/// `WatchStartResult { started: false, debounce_ms }` otherwise.
///
/// Errors when the server is not `Ready` (no served snapshot to keep
/// fresh in `Indexing`; no snapshot at all in `Failed`), or when
/// `notify::RecommendedWatcher` initialization fails. See
/// `mcp-orchestrated-indexing` and design.md §D6 / §D6a / §D6b.
pub fn watch_start(
    state: &Arc<ServerState>,
    _: WatchStartArgs,
) -> Result<SingleResponse<WatchStartResult>, McpError> {
    // State precondition: only meaningful in Ready.
    {
        let g = state.lifecycle.read().map_err(|e| {
            McpError::new(
                McpErrorCode::InternalError,
                format!("lifecycle lock poisoned: {e}"),
            )
        })?;
        let kind = g.kind();
        if kind != crate::state::StateKind::Ready {
            return Err(McpError::new(
                McpErrorCode::InvalidInput,
                format!(
                    "watch_start: server is `{}`, not `ready`; poll get_index_status and retry",
                    kind.as_str()
                ),
            ));
        }
    }

    // Hold the watcher mutex across the entire start so concurrent
    // `watch_start` callers serialize: the first one constructs and
    // installs the handle; the second sees `is_some()` and returns
    // `started: false` without wasting a `notify::Watcher::new`. The
    // mutex is held for the duration of `crate::watcher::start`, which
    // is a few ms of syscalls (notify channel + recursive watch on
    // workspace root).
    let debounce_ms = state.config.mcp.watch_debounce_ms;
    let mut g = state.watcher.lock().map_err(|e| {
        McpError::new(
            McpErrorCode::InternalError,
            format!("watcher mutex poisoned: {e}"),
        )
    })?;
    if g.is_some() {
        return Ok(SingleResponse::found(WatchStartResult {
            started: false,
            debounce_ms,
        }));
    }

    // Start a fresh watcher. notify::Watcher::new failures surface as
    // an MCP error; the watcher state stays `Off` on failure.
    let (result, handle) = crate::watcher::start(state).map_err(|e| {
        McpError::new(
            McpErrorCode::InternalError,
            format!("watch_start: notify init failed: {e}"),
        )
    })?;
    *g = Some(handle);
    Ok(SingleResponse::found(result))
}

#[derive(Debug, Deserialize, Default, Serialize, JsonSchema)]
pub struct WatchStopArgs {}

/// Outcome of a `watch_stop` call.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct WatchStopResult {
    /// True if a watcher was running and was stopped; false if no
    /// watcher was running (idempotent no-op).
    pub stopped: bool,
}

/// The `watch_stop` tool. Idempotent: aborts the debounce task and
/// drops the `notify` watcher if one is running; succeeds with
/// `stopped: false` otherwise. Permitted in any server state.
pub fn watch_stop(
    state: &ServerState,
    _: WatchStopArgs,
) -> Result<SingleResponse<WatchStopResult>, McpError> {
    let stopped = crate::watcher::stop(state);
    Ok(SingleResponse::found(WatchStopResult { stopped }))
}

#[cfg(test)]
mod tests {
    use super::{clamp_timeout_ms, degraded_fields, embed_stage_str, WAIT_DEFAULT_MS, WAIT_MAX_MS};
    use crate::state::EmbedStage;

    /// Build a `SnapshotMeta` from a partial JSON object — serde defaults
    /// fill every field the test doesn't care about.
    fn meta(value: serde_json::Value) -> kenn_indexer::SnapshotMeta {
        serde_json::from_value(value).expect("meta")
    }

    #[test]
    fn degraded_fields_none_meta_is_all_empty() {
        let d = degraded_fields(None);
        assert!(d.run_status.is_none());
        assert!(d.failed_projects.is_empty() && d.warnings.is_empty());
        assert_eq!((d.failed_count, d.warning_count), (0, 0));
    }

    #[test]
    fn degraded_fields_clean_success_omits_everything() {
        let m = meta(serde_json::json!({
            "timestamp": "2026-07-07T00-00-00Z", "status": "success",
            "documents": 1, "symbols": 1, "definitions": 0, "edges": 0,
        }));
        let d = degraded_fields(Some(&m));
        assert!(d.run_status.is_none(), "clean run suppresses run_status");
        assert!(d.failed_projects.is_empty() && d.warnings.is_empty());
    }

    #[test]
    fn degraded_fields_partial_reports_true_counts_with_raw_list() {
        let m = meta(serde_json::json!({
            "timestamp": "2026-07-07T00-00-00Z", "status": "partial",
            "documents": 1, "symbols": 1, "definitions": 0, "edges": 0,
            "failed_projects": ["csharp: msbuild failed", "swift: build failed"],
            "failed_overflow": 32,
        }));
        let d = degraded_fields(Some(&m));
        assert_eq!(d.run_status.as_deref(), Some("partial"));
        assert_eq!(d.failed_count, 34, "2 listed + 32 overflow");
        // The structured array carries the RAW bounded entries only — the
        // `+N more` display marker must NOT leak into machine-readable data;
        // the overflow lives in failed_count.
        assert_eq!(d.failed_projects.len(), 2);
        assert!(
            !d.failed_projects.iter().any(|s| s.contains("more")),
            "no display marker in the structured array: {:?}",
            d.failed_projects
        );
    }

    #[test]
    fn degraded_fields_regression_only_is_not_clean() {
        // A success run whose only diagnostic is a metric regression must
        // NOT be reported clean (parity with `kenn status`, which prints it).
        let m = meta(serde_json::json!({
            "timestamp": "2026-07-07T00-00-00Z", "status": "success",
            "documents": 1, "symbols": 1, "definitions": 0, "edges": 0,
            "regression_warnings": [
                {"metric": "symbols", "previous": 1000, "current": 400, "drop_pct": 60}
            ],
        }));
        let d = degraded_fields(Some(&m));
        assert_eq!(d.run_status.as_deref(), Some("success"));
        assert_eq!(d.regression_count, 1);
        assert!(d.failed_projects.is_empty() && d.warnings.is_empty());
    }

    #[test]
    fn degraded_fields_success_with_warnings_reports_them() {
        let m = meta(serde_json::json!({
            "timestamp": "2026-07-07T00-00-00Z", "status": "success",
            "documents": 1, "symbols": 1, "definitions": 0, "edges": 0,
            "warnings": ["swift: 3 stale index-store units kept"],
        }));
        let d = degraded_fields(Some(&m));
        assert_eq!(d.run_status.as_deref(), Some("success"));
        assert_eq!(d.warning_count, 1);
        assert!(d.failed_projects.is_empty());
    }

    #[test]
    fn embed_stage_maps_to_status_string() {
        assert_eq!(embed_stage_str(EmbedStage::Building), "embedding");
        assert_eq!(embed_stage_str(EmbedStage::Ready), "ready");
        assert_eq!(embed_stage_str(EmbedStage::Disabled), "disabled");
        assert_eq!(embed_stage_str(EmbedStage::Degraded), "degraded");
    }

    #[test]
    fn clamp_timeout_defaults_and_caps() {
        assert_eq!(clamp_timeout_ms(None), WAIT_DEFAULT_MS);
        assert_eq!(clamp_timeout_ms(Some(1_000)), 1_000);
        assert_eq!(clamp_timeout_ms(Some(WAIT_MAX_MS + 1)), WAIT_MAX_MS);
        assert_eq!(clamp_timeout_ms(Some(u64::MAX)), WAIT_MAX_MS);
        assert_eq!(clamp_timeout_ms(Some(0)), 0);
    }
}
