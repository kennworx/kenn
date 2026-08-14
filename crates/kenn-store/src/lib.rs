//! `.kenn/` store layout, snapshot lifecycle, GC, rollback, staleness,
//! worktree fallback, and the storage surface (`api::Reader`,
//! `api::WriteBatch`) for the kenn storage layer.
//!
//! Implements the `index-store-layout`, `index-lifecycle`,
//! `index-store-staleness`, `index-store-worktree-fallback`, and
//! `storage-backend-abstraction` capabilities. The concrete backend —
//! a single storage engine, `SQLite` — lives under the private `db` module
//! and is reached only through [`open_writer`] / [`open_reader`].

pub mod api;
mod clock;
mod db;
pub mod embed;
pub mod git;
pub mod layout;
pub mod lifecycle;
mod meta;
pub mod metrics;
mod open;
pub mod readers;
pub mod staleness;
pub mod worktree;

pub use api::types::{
    AggregateEdgeRow, AggregateNodeRow, AnalysisFlatCommunityRow, AnalysisNodeMembershipRow,
    BlendedFileRow, BlendedHit, BlendedSymbolRow, CodeSymbolHit, DbError, DefLineRow, DefRow,
    FileRow, Finding, FindingHit, FoundSymbolRow, LinkDiagnosticRow, MatchKind, PackageRow,
    RankedSymbolRow, RowNarrow, StatRow, SymbolBodyRow, SymbolDocsRow, SymbolRow, SymbolSurfaceRow,
    WriterOptions,
};
pub use clock::Timestamp;

// The findings store (`openspec/changes/findings-backend`). DELIBERATE
// DEVIATION from task 7.1's literal "add to the `Reader` trait" wording:
// findings are workspace-durable with a lifecycle independent of the
// per-index-run snapshot, so the standalone `FindingsStore` IS the
// surface rather than a method set hung off the snapshot `Reader`. This
// satisfies task 7.1's intent — the findings API is reachable from the
// crate root.
pub use db::{
    embed_pending, finding_is_stale, read_embed_error, reembed, stage_findings_for_publish, Anchor,
    AnchorEvent, AnchorHealth, BrokenAnchors, CodeGraphNodeResolver, CodeNodeResolver, DbConn,
    DbReader, DbWriter, DriftedAnchors, FindingsStore, Outcome, ReembedReport, UnverifiedClaim,
};
pub use embed::sidecar::{
    code_generation_dir, current_model_id, findings_generation_dir, gc_vector_cache,
    promote_segs_to_packs, sidecar_dirs, VectorsGcReport,
};
pub use embed::{
    init_shared_embedder, release_shared_embedder, shared_embedder, EmbeddingProducer,
    LazyEmbedder, LlamaEmbedder, SharedEmbedder,
};
pub use layout::{Layout, RunMeta, Store};
pub use lifecycle::{
    begin_indexing, current_state, decide_startup_state, gc, list_completed_runs, recover,
    rollback, BeginError, IndexingHandle, LifecycleState, PublishError, RecoveryError,
    RecoveryReport, RollbackError, StartupDecision,
};
pub use meta::{check_backend_marker, check_schema_version, ACTIVE_BACKEND, STORE_SCHEMA_VERSION};
pub use metrics::{compute_diff, MetricSnapshot, RegressionWarning};
pub use open::{open_reader, open_writer, reader_from_writer};
pub use staleness::{compute_staleness_key, file_content_sha, DirtyFile, StalenessKey};
pub use worktree::{open_for_read, resolve_main_worktree, ReadContext, ReadSource};
