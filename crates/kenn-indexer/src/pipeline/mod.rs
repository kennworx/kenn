//! The 4-phase indexing orchestrator: **prepare → ingest → aggregate →
//! finalize**.
//!
//! - **Prepare** — preflight that every configured ingester CLI is
//!   available, and discover the package layout. The backend was
//!   constructed by the caller's `open_writer` and is handed in.
//! - **Ingest** — one OS thread per language ingester. Each parses in
//!   parallel, interns into its own `short_id` partition, builds
//!   records, and appends them **directly** to the Lance datasets
//!   through its own [`BatchSink`] — no DB-writer thread, no channel
//!   (design D9). Lance's optimistic-concurrency commit guard resolves
//!   the concurrent appends.
//! - **Aggregate** — roll the per-symbol graph up into `aggregate_*` /
//!   `analysis_*`.
//! - **Finalize** — compact + index every dataset and build the
//!   knowledge store.
//!
//! The Lance store is async; `run_pipeline` runs inside a caller's
//! `spawn_blocking`, so it owns a private `tokio::runtime::Runtime` and
//! drives every async append from the (plain, runtime-free) ingester OS
//! threads via `Handle::block_on`.
//!
//! Module layout:
//! - [`api`] — the public types + orchestrators + preflight.
//! - [`ingest`] — per-unit SCIP/JSONL ingest + the kenn-dotnet retry path.

mod api;
mod ingest;

#[cfg(test)]
mod tests;

pub use api::{
    no_op_hook, run_pipeline, run_pipeline_with_progress, IngestUnit, PipelineError,
    PostAggregateHook, ProgressEvent,
};

pub(crate) use ingest::{ingest_jsonl_driver, ingest_scip_driver};
