//! Indexing pipeline (producer) and end-to-end orchestration.
//!
//! Consumes per-language indexer output and produces records conforming
//! to the `kenn-model` schema. Two formats are supported:
//!
//! - **JSONL** (kenn-dotnet, future kenn-* indexers): streamed directly
//!   from the indexer subprocess's stdout, no intermediate file.
//! - **SCIP** (scip-typescript, scip-rust, scip-python, scip-go): the
//!   indexer writes `.scip` to disk; we read and transform.
//!
//! `pipeline::run_pipeline` dispatches on the driver's `DriverOutcome`
//! variant: `Jsonl` → `transform_jsonl::ingest_jsonl_into_sink`, `Scip` →
//! `pipeline::ingest_scip_into_sink` (existing edge-derivation flow).
//!
//! `workflow::index_workspace` orchestrates the pipeline together with
//! `kenn-store`'s lifecycle / store / staleness machinery for both
//! `kenn index` (CLI) and `kenn mcp` (server).

use std::sync::LazyLock;

/// `KENN_BENCH` cached once — env-var lookups go through a process-wide
/// mutex on macOS, which matters in per-frame and per-checkpoint hot
/// paths. The env is set before the indexer starts; mid-run mutation
/// would not be honoured anyway.
pub(crate) static BENCH_ENABLED: LazyLock<bool> =
    LazyLock::new(|| std::env::var_os("KENN_BENCH").is_some());

pub mod aggregate;
pub mod atlas;
pub mod canonicalize;
pub mod code_sql;
pub mod css;
pub mod docker;
pub mod driver;
pub mod edge;
pub mod enclosing;
pub mod html;
pub mod markdown;
pub mod merge;
pub mod package_layout;
pub mod parse;
pub mod parse_jsonl;
pub mod pipeline;
pub mod provision;
pub mod pubid;
pub mod relpath;
pub mod report;
pub mod sink;
pub mod snapshot;
pub mod sql;
pub mod text;
pub mod transform;
pub mod transform_jsonl;
pub mod workflow;
pub mod xml;
pub mod xml_sql;

pub use canonicalize::{
    discover_other_worktrees, CanonicalizeError, Workspace, WorkspaceRelativePath,
};
pub use driver::KennDotnet;
pub use pipeline::{run_pipeline, run_pipeline_with_progress, PipelineError};
pub use report::{render_toolchains, ProvisionResult, RunReport, RunStatus, ToolchainVersion};
pub use snapshot::{
    aggregate_counts, build_snapshot_meta, persist_run_artifacts, RegressionWarning,
    SnapshotCounts, SnapshotMeta, SNAPSHOT_META_FILE,
};
pub use workflow::{
    build_workspace, configure_runner, index_workspace, language_claimed_extensions, WorkflowError,
    WorkflowOutcome,
};
