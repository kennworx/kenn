//! MCP-side indexing orchestration.
//!
//! Pieces, all driven from [`start_background_indexing`]:
//!
//! - **Cold-start** ([`run_startup_decision`]) — Skip vs Reindex on
//!   boot; on success transitions to `Ready`, on failure to `Failed`.
//! - **Hot-reload** ([`handle_live_event`]) — driven by the file
//!   watcher's `live`-pointer event (not a timer): resolves `live` and,
//!   if it differs from the served run, atomically swaps the
//!   [`crate::state::ReaderBinding`]. Self-publish is deduped
//!   (`resolved(live) == current`). See watcher-driven-staleness D3.
//! - **Background reindex** ([`spawn_background_reindex`]) — driven by
//!   the watcher's debounce trigger, the `reindex` tool, or a synthetic
//!   event from `Ready`; reads stay served from the prior snapshot
//!   throughout. On success it swaps to its own published run itself,
//!   stamping `run_event_seq` with the counter captured at the reindex's
//!   start (the self-publish swap, D4). Failures clear `Ready.reindex`
//!   and stay `Ready` (Decision 5).
//! - **Staleness backstop** ([`start_staleness_backstop_task`]) — a
//!   low-frequency `spawn_blocking` git key-compare that catches dropped
//!   watcher events (and, after `watch_stop`, is the only freshness
//!   mechanism). On mismatch it synthesizes an event (D5). Also a safety
//!   net for missed `live` events.
//! - **Recovery** ([`spawn_recovery_pipeline`]) — driven by the
//!   `reindex` tool from `Failed`; reuses the cold-start path.
//!
//! `is_stale` is an event-seq generation comparison
//! (`last_event_seq > run_event_seq`), read by `get_index_status` with
//! no git work on the call path (D1/D4).
//!
//! Also bridges the pipeline's progress callback to the rmcp
//! logging-notification stream. See `mcp-orchestrated-indexing` for the
//! capability contract.
//!
//! Module layout:
//! - [`orchestrate`] — cold-start/reindex lifecycle + ready-binding + swap.
//! - [`events`] — hot-reload, staleness backstop, recovery spawners.
//! - [`roots`] — workspace-roots discovery + rebind.

mod events;
mod orchestrate;
mod roots;

#[cfg(test)]
mod tests;

pub use events::{
    handle_live_event, poll_once, spawn_background_reindex, spawn_recovery_pipeline,
    start_staleness_backstop_task,
};
pub use orchestrate::{autostart_watcher, code_updated_payload, start_background_indexing};
pub use roots::{pick_first_file_root, rebind_workspace, resolve_roots_and_maybe_rebind};

pub(crate) use events::{set_failed, startup_seed};
pub(crate) use orchestrate::{
    emit_code_updated, open_binding, ready_from_parts, run_startup_decision, swap_to_snapshot,
};
pub(crate) use roots::reload_kenn_toml;

#[cfg(test)]
pub(crate) use orchestrate::format_progress;
#[cfg(test)]
pub(crate) use roots::{decide_roots_resolution, RootsResolution};
