//! `Reader` trait — async surface every backend implements for MCP and
//! CLI read traffic.
//!
//! Single trait, methods grouped by concern: symbol fetch, defs / def
//! lines, files / packages, docs, graph traversal, text search, hybrid
//! search, catalog. Backends that cannot serve a particular method
//! return `DbError::Backend("unsupported: ...")`; callers cannot tell
//! the difference at the trait level.

use kenn_model::ShortId;

use crate::api::types::{
    AggregateEdgeRow, AggregateNodeRow, AnalysisAnchoredCommunityRow, AnalysisFlatCommunityRow,
    AnalysisGodNodeRow, AnalysisNodeMembershipRow, BlendedHit, BlendedSymbolRow, DbError,
    DefLineRow, DefRow, FileRow, FoundSymbolRow, PackageRow, RankedSymbolRow, SymbolDocsRow,
    SymbolRow,
};

/// Storage-side reader contract. The MCP server holds an `impl Reader`
/// behind an `Arc` and calls these methods to serve tool requests.
///
/// All methods are async because the canonical (Surreal) backend is
/// async-native. Backends with sync underlying APIs (the future
/// `tantivy + redb + hnsw_rs` backend) wrap calls in
/// `tokio::task::spawn_blocking` at the impl boundary.
///
/// `search_symbols_blended` returns a single ranked list — the trait
/// does NOT expose the BM25 candidate list and the vector kNN list
/// separately. Fusion (native engine blend, RRF, weighted, …) is the
/// backend's choice.
pub trait Reader: Send + Sync {
    // ── symbol fetch ────────────────────────────────────────────────

    fn fetch_symbol_pub_id(
        &self,
        short_id: ShortId,
    ) -> impl std::future::Future<Output = Result<Option<String>, DbError>> + Send;

    fn fetch_symbol(
        &self,
        language: &str,
        pub_id: &str,
    ) -> impl std::future::Future<Output = Result<Option<SymbolRow>, DbError>> + Send;

    fn fetch_symbol_by_short_id(
        &self,
        short_id: ShortId,
    ) -> impl std::future::Future<Output = Result<Option<SymbolRow>, DbError>> + Send;

    fn fetch_symbol_docs_row(
        &self,
        symbol_short_id: ShortId,
    ) -> impl std::future::Future<Output = Result<Option<SymbolDocsRow>, DbError>> + Send;

    // ── defs / def lines ────────────────────────────────────────────

    fn fetch_defs(
        &self,
        symbol_short_id: ShortId,
    ) -> impl std::future::Future<Output = Result<Vec<DefRow>, DbError>> + Send;

    fn fetch_def_lines(
        &self,
        symbol_short_id: ShortId,
    ) -> impl std::future::Future<Output = Result<Vec<DefLineRow>, DbError>> + Send;

    // ── files / packages ────────────────────────────────────────────

    fn fetch_package(
        &self,
        short_id: ShortId,
    ) -> impl std::future::Future<Output = Result<Option<PackageRow>, DbError>> + Send;

    fn fetch_file_path(
        &self,
        short_id: ShortId,
    ) -> impl std::future::Future<Output = Result<Option<String>, DbError>> + Send;

    fn fetch_file_short_id(
        &self,
        path: &str,
    ) -> impl std::future::Future<Output = Result<Option<ShortId>, DbError>> + Send;

    // ── graph traversal ─────────────────────────────────────────────

    fn find_at_location(
        &self,
        file_short_id: ShortId,
        line: u32,
    ) -> impl std::future::Future<Output = Result<Vec<SymbolRow>, DbError>> + Send;

    fn list_inbound(
        &self,
        target_short_id: ShortId,
        relation: &str,
        limit: u32,
        cursor_after: Option<ShortId>,
        include_external: bool,
        include_tests: bool,
    ) -> impl std::future::Future<Output = Result<(Vec<SymbolRow>, u64), DbError>> + Send;

    fn list_outbound(
        &self,
        source_short_id: ShortId,
        relation: &str,
        limit: u32,
        cursor_after: Option<ShortId>,
        include_external: bool,
        include_tests: bool,
    ) -> impl std::future::Future<Output = Result<(Vec<SymbolRow>, u64), DbError>> + Send;

    fn list_module_files(
        &self,
        module_short_id: ShortId,
        limit: u32,
        cursor_after: Option<ShortId>,
    ) -> impl std::future::Future<Output = Result<(Vec<FileRow>, u64), DbError>> + Send;

    // ── text search ─────────────────────────────────────────────────

    fn find_symbol_tiered(
        &self,
        name: &str,
        limit: u32,
        include_external: bool,
        include_tests: bool,
    ) -> impl std::future::Future<Output = Result<Vec<FoundSymbolRow>, DbError>> + Send;

    fn search_symbols_by_name(
        &self,
        query: &str,
        limit: u32,
        cursor_score: Option<f32>,
        cursor_short_id: Option<ShortId>,
        include_external: bool,
        include_tests: bool,
    ) -> impl std::future::Future<Output = Result<(Vec<RankedSymbolRow>, u64), DbError>> + Send;

    // ── hybrid search ───────────────────────────────────────────────

    /// BM25 + vector blended search returning a single ranked list of up
    /// to `target_total` symbol rows. File-level-doc hits are excluded —
    /// use [`Self::search_blended_hits`] for the file-inclusive variant.
    /// The trait does NOT expose the unfused BM25 / vector candidates;
    /// fusion is the backend's choice. Order is `(score DESC, len(name)
    /// ASC, id ASC)`. Callers that paginate cache the returned Vec and
    /// slice it themselves; the reader does NOT paginate.
    /// `query_vec` is the caller-supplied query embedding; `None` means
    /// "embedding unavailable" and the search degrades to BM25-only (the
    /// vector arm contributes no hits). Callers run the embed step
    /// themselves so the store doesn't depend on `kenn_embed`.
    fn search_symbols_blended(
        &self,
        query: &str,
        query_vec: Option<&[f32]>,
        target_total: u32,
        include_external: bool,
        include_tests: bool,
    ) -> impl std::future::Future<Output = Result<Vec<BlendedSymbolRow>, DbError>> + Send;

    /// Like [`Self::search_symbols_blended`], but interleaves file-level
    /// doc hits ([`BlendedHit::File`]) with symbol hits by score — the
    /// surface behind the `search_symbols` MCP tool. A file hit and a
    /// symbol hit can share an id number (independent id spaces), so the
    /// variant — not the id — says which dataset a hit came from.
    fn search_blended_hits(
        &self,
        query: &str,
        query_vec: Option<&[f32]>,
        target_total: u32,
        include_external: bool,
        include_tests: bool,
    ) -> impl std::future::Future<Output = Result<Vec<BlendedHit>, DbError>> + Send;

    // ── bulk scan (analysis) ────────────────────────────────────────

    /// Stream every symbol row in the snapshot. Used by `kenn-analyze`
    /// for whole-graph analysis; not on the MCP hot path.
    fn scan_symbols(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<SymbolRow>, DbError>> + Send;

    /// Stream every `(source, target)` pair for the given relation. Dedupes
    /// identical pairs that may exist when the kind carries a payload
    /// (e.g. `field_access` Read/Write). Used by `kenn-analyze`.
    fn scan_edges(
        &self,
        relation: &str,
    ) -> impl std::future::Future<Output = Result<Vec<(ShortId, ShortId)>, DbError>> + Send;

    /// Stream every row from the snapshot's `aggregate_nodes` table. Returns
    /// an empty vector when the snapshot pre-dates the aggregate-graph
    /// schema (no error). Backed by the indexer's `end_run` aggregation
    /// pass; used by `kenn-analyze` and any future MCP tools that want
    /// the rolled-up graph without re-projecting from per-symbol tables.
    fn scan_aggregate_nodes(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<AggregateNodeRow>, DbError>> + Send;

    /// Stream every row from the snapshot's `aggregate_edges` table. Each
    /// row is one undirected aggregated edge of a specific kind with its
    /// total weight. Returns empty on pre-aggregate snapshots.
    fn scan_aggregate_edges(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<AggregateEdgeRow>, DbError>> + Send;

    // ── persisted analysis ──────────────────────────────────────────
    //
    // The four `scan_analysis_*` methods read tables written by the
    // index pipeline's analysis phase. All four return `Ok(vec![])`
    // (not an error) on snapshots indexed before the feature shipped
    // or with `[index] persist_analysis = false`; callers detect
    // missing data by checking emptiness, not by error code.

    /// Top-N god-nodes per filter (live / test / external). Filter is
    /// the stringified `GodNodeFilter::db_name()`. Rows are sorted by
    /// `(filter, rank)`.
    fn scan_analysis_god_nodes(
        &self,
        filter: &str,
    ) -> impl std::future::Future<Output = Result<Vec<AnalysisGodNodeRow>, DbError>> + Send;

    /// Flat-Louvain communities. Rows sorted by `community_id`.
    fn scan_analysis_flat_communities(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<AnalysisFlatCommunityRow>, DbError>> + Send;

    /// Anchored-Louvain recursive hierarchy. Rows sorted by
    /// `(anchor_id, depth, community_id)` so a caller can rebuild a
    /// per-anchor tree in one linear scan.
    fn scan_analysis_anchored_hierarchy(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<AnalysisAnchoredCommunityRow>, DbError>> + Send;

    /// Per-aggregate-node community lookup. Rows sorted by `short_id`.
    fn scan_analysis_node_membership(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<AnalysisNodeMembershipRow>, DbError>> + Send;

    // ── catalog ─────────────────────────────────────────────────────

    fn distinct_languages(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<String>, DbError>> + Send;

    fn distinct_packages(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<String>, DbError>> + Send;

    fn count_table(
        &self,
        table: &str,
    ) -> impl std::future::Future<Output = Result<u64, DbError>> + Send;
}
