//! `IndexStatus` — the `get_index_status` payload.
//!
//! Lives with the daemon, not with the query layer. Every field describes the
//! *server*: which lifecycle phase it is in, whether its file watcher is
//! running, what the run that produced the served snapshot reported. None of it
//! is a fact about the indexed code, so none of it belongs in `kenn-query`.

use serde::{Deserialize, Serialize};

/// Per-server lifecycle state surfaced to MCP clients via the
/// `get_index_status` tool.
///
/// Always carries `state` (`"indexing" | "embedding" | "ready" | "disabled"
/// | "failed"`); other fields are populated based on the state. Agents should
/// branch on `state` rather than relying on individual fields being present.
/// The progression is `indexing → embedding → ready`. **Structural queries
/// (`find_symbol`, `list_callers`, …) work from `embedding` onward** — only
/// vector queries (`find_similar`, `semantic_search`) wait for `ready`; do not
/// block a structural query on `ready`. `disabled` = graph ready, no embedder,
/// so vectors will not be built (lexical-only).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IndexStatus {
    /// `"indexing" | "embedding" | "ready" | "disabled" | "failed"`. The
    /// remaining fields are populated based on this value. `embedding` and
    /// `disabled` both mean the code graph is ready (structural tools serve);
    /// they differ only in whether vectors are still building (`embedding`) or
    /// will never be built for lack of an embedder (`disabled`).
    pub state: String,
    /// Absolute filesystem path of the workspace the MCP server is
    /// indexing. Useful for agents to confirm they're talking to the
    /// expected workspace / worktree (especially when multiple agents
    /// run in parallel against different branches).
    pub workspace_root: String,
    /// Snapshot id (hex) — populated only when `state == "ready"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    /// ISO 8601 timestamp the snapshot was published — populated only
    /// when `state == "ready"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_at: Option<String>,
    /// Per-spec; reserved for the future-watcher driven update flow.
    /// Always `false` for now.
    #[serde(default)]
    pub is_stale: bool,
    /// Always `false` for now (no in-process reindex during serving in
    /// the foundation change). Kept for forward-compat.
    #[serde(default)]
    pub reindex_in_progress: bool,
    /// True when the snapshot we're reading lives in the parent
    /// worktree because the local one was unavailable.
    #[serde(default)]
    pub fallback_from_parent_worktree: bool,
    /// Pipeline progress — populated only when `state == "indexing"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<IndexStatusProgress>,
    /// Pipeline error — populated only when `state == "failed"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Current state of the in-process file watcher. `Off` (not running),
    /// `Idle` (running, no pending debounce), or `Debouncing` (running,
    /// an event has landed and a reindex trigger is scheduled).
    #[serde(default = "default_watcher_state")]
    pub watcher: crate::state::WatcherState,
    /// Aggregate status of the run that produced the served snapshot —
    /// `"success" | "partial"`. **Presence** is the signal: this field (and
    /// the ones below) appear exactly when the run was NOT clean — i.e. it
    /// produced at least one failed project, warning, or metric regression.
    /// The **value** is the aggregate status, which can be `"success"` for a
    /// run that succeeded but carries warnings or regressions. A clean run
    /// omits all of these, so the happy-path payload is unchanged. A
    /// fully-failed run publishes no snapshot and surfaces via
    /// `state: "failed"` instead (never here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_status: Option<String>,
    /// Per-language / per-project failure attributions from the served
    /// snapshot's run — the RAW bounded list (no `+N more` display marker).
    /// `failed_count` is the true total including dropped overflow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_projects: Vec<String>,
    /// True number of failed projects (bounded list length + overflow).
    /// Omitted when zero.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub failed_count: u64,
    /// Status-neutral producer diagnostics from the served snapshot's run
    /// (e.g. stale index-store units kept) — the RAW bounded list.
    /// `warning_count` is the true total.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// True number of warnings (bounded list length + overflow). Omitted
    /// when zero.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub warning_count: u64,
    /// Number of metric regressions the run recorded against the prior
    /// snapshot (surfaced by `kenn index`; the MCP self-reindex path
    /// records none). Omitted when zero. Detail (which metric, old→new) is
    /// available from `kenn status`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub regression_count: u64,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if takes &T"
)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn default_watcher_state() -> crate::state::WatcherState {
    crate::state::WatcherState::Off
}

/// Coarse-grained progress info exposed in `IndexStatus.progress`.
/// Values accumulate as the pipeline runs; `phase` tracks the latest
/// pipeline milestone (e.g. `"ingest"`, `"flush_stubs"`, `"end_run"`).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IndexStatusProgress {
    pub phase: String,
    pub files_seen: u64,
    pub symbols_seen: u64,
    pub edges_seen: u64,
}
