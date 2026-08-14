//! `DbReader` / `DbConn` — the async, `Send + Sync` read handles over one
//! snapshot, and `DbWriter` — the write handle the indexer drives.
//!
//! `DbReader` wraps a [`SqliteReader`] (an async-sqlite connection pool). Its
//! [`Reader`] methods dispatch each query onto a pooled background-thread
//! connection, so blocking `SQLite` never runs on a runtime worker and concurrent
//! reads parallelize. `DbConn` is a clone of the same handle (kept as a
//! distinct name for the per-query call sites).
//!
//! `SqliteWriter` owns plain `rusqlite::Connection`s (`!Sync`), so `DbWriter`
//! wraps it in a `Mutex`: each method locks, does its synchronous work, and
//! drops the guard before returning, so no guard is held across an `.await` and
//! the futures are `Send` (the indexer moves the writer into a blocking task).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use kenn_model::{
    AggregateEdgeRecord, AggregateNodeRecord, AnalysisAnchoredCommunityRecord,
    AnalysisFlatCommunityRecord, AnalysisGodNodeRecord, AnalysisNodeMembershipRecord, EdgeKind,
    FileRecord, PackageRecord, SymbolRecord,
};

use crate::api::types::{
    AggregateEdgeRow, AggregateNodeRow, AnalysisAnchoredCommunityRow, AnalysisFlatCommunityRow,
    AnalysisGodNodeRow, AnalysisNodeMembershipRow, BlendedHit, BlendedSymbolRow, DbError,
    DefLineRow, DefRow, FileRow, FoundSymbolRow, PackageRow, RankedSymbolRow, SymbolBodyRow,
    SymbolDocsRow, SymbolRow, SymbolSurfaceRow, WriterOptions,
};
use crate::api::{Reader, RowNarrow, WriteBatch};
use kenn_model::ShortId;

use super::reader::SqliteReader;
use super::writer::SqliteWriter;

/// Async read handle over one snapshot — a clonable handle to the snapshot's
/// connection pool. Exposes the [`Reader`] trait plus the two non-trait extras
/// `kenn-mcp` calls. Cloning (via [`Self::connect`]) is cheap: the pool is
/// `Arc`-backed and shared.
pub struct DbReader(SqliteReader);

/// A per-query read handle. Now identical to [`DbReader`] — both are clones of
/// the same pool handle — kept as a distinct name for the MCP query call sites
/// that hold one for the duration of a `tools/call`.
pub type DbConn = DbReader;

impl DbReader {
    /// Open a reader (connection pool) against a published snapshot directory.
    pub(crate) async fn open(snapshot: &Path) -> Result<Self, DbError> {
        Ok(Self(SqliteReader::open(snapshot).await?))
    }

    /// A cheap clone of this pool handle (no I/O). Named `connect` for the MCP
    /// call sites that historically opened a per-query session here.
    pub fn connect(&self) -> Result<DbConn, DbError> {
        Ok(Self(self.0.clone()))
    }

    pub async fn find_similar_symbols(
        &self,
        source: ShortId,
        limit: u32,
        include_external: bool,
        include_tests: bool,
    ) -> Result<Option<Vec<RankedSymbolRow>>, DbError> {
        self.0
            .find_similar_symbols(source, limit, include_external, include_tests)
            .await
    }

    /// Code FILE rows whose basename equals `basename` — md→code link
    /// resolution (index-markdown Group 6). Not on the [`Reader`] trait: it
    /// serves the indexer's post-code barrier, not the MCP hot path.
    pub async fn files_by_basename(&self, basename: &str) -> Result<Vec<FileRow>, DbError> {
        self.0.files_by_basename(basename).await
    }

    /// Code SYMBOL rows whose short name equals `name` — md→code link
    /// resolution (index-markdown Group 6).
    pub async fn symbols_by_short_name(
        &self,
        name: &str,
    ) -> Result<Vec<crate::api::types::CodeSymbolHit>, DbError> {
        self.0.symbols_by_short_name(name).await
    }

    /// Non-exact markdown links for the `check_links` MCP tool (the read path
    /// for the `link_grade` edge column — index-markdown Group 7). Optionally
    /// filtered to `grade_codes`, capped at `limit`, with the full match count.
    pub async fn scan_link_diagnostics(
        &self,
        grade_codes: Option<Vec<u8>>,
        limit: u32,
    ) -> Result<(Vec<crate::api::types::LinkDiagnosticRow>, u64), DbError> {
        self.0.scan_link_diagnostics(grade_codes, limit).await
    }

    /// Dead-CSS findings for the `check_css` MCP tool (orphan classes /
    /// stylesheets — index-css Group 9). Bounded at `limit` rows with the full
    /// per-category counts.
    pub async fn scan_css_health(
        &self,
        want_classes: bool,
        want_sheets: bool,
        limit: u32,
    ) -> Result<
        (
            Vec<crate::api::types::CssHealthRow>,
            crate::api::types::CssHealthCounts,
        ),
        DbError,
    > {
        self.0
            .scan_css_health(want_classes, want_sheets, limit)
            .await
    }

    pub async fn code_node_resolver(
        &self,
    ) -> Result<super::super::findings::CodeGraphNodeResolver, DbError> {
        self.0.code_node_resolver().await
    }

    pub async fn stats(&self) -> Result<Vec<crate::api::types::StatRow>, DbError> {
        self.0.stats().await
    }
}

impl Reader for DbReader {
    async fn fetch_symbol_pub_id(&self, short_id: ShortId) -> Result<Option<String>, DbError> {
        Reader::fetch_symbol_pub_id(&self.0, short_id).await
    }
    async fn fetch_symbol(
        &self,
        language: &str,
        pub_id: &str,
    ) -> Result<Option<SymbolRow>, DbError> {
        Reader::fetch_symbol(&self.0, language, pub_id).await
    }
    async fn fetch_symbol_by_short_id(
        &self,
        short_id: ShortId,
    ) -> Result<Option<SymbolRow>, DbError> {
        Reader::fetch_symbol_by_short_id(&self.0, short_id).await
    }
    async fn fetch_symbol_docs_row(
        &self,
        symbol_short_id: ShortId,
    ) -> Result<Option<SymbolDocsRow>, DbError> {
        Reader::fetch_symbol_docs_row(&self.0, symbol_short_id).await
    }
    async fn fetch_defs(&self, symbol_short_id: ShortId) -> Result<Vec<DefRow>, DbError> {
        Reader::fetch_defs(&self.0, symbol_short_id).await
    }
    async fn fetch_def_lines(&self, symbol_short_id: ShortId) -> Result<Vec<DefLineRow>, DbError> {
        Reader::fetch_def_lines(&self.0, symbol_short_id).await
    }
    async fn fetch_package(&self, short_id: ShortId) -> Result<Option<PackageRow>, DbError> {
        Reader::fetch_package(&self.0, short_id).await
    }
    async fn fetch_file_path(&self, short_id: ShortId) -> Result<Option<String>, DbError> {
        Reader::fetch_file_path(&self.0, short_id).await
    }
    async fn fetch_file_short_id(&self, path: &str) -> Result<Option<ShortId>, DbError> {
        Reader::fetch_file_short_id(&self.0, path).await
    }
    async fn find_at_location(
        &self,
        file_short_id: ShortId,
        line: u32,
    ) -> Result<Vec<SymbolRow>, DbError> {
        Reader::find_at_location(&self.0, file_short_id, line).await
    }
    async fn list_inbound(
        &self,
        target_short_id: ShortId,
        relation: &str,
        limit: u32,
        cursor_after: Option<ShortId>,
        narrow: &RowNarrow,
    ) -> Result<(Vec<SymbolRow>, u64), DbError> {
        Reader::list_inbound(
            &self.0,
            target_short_id,
            relation,
            limit,
            cursor_after,
            narrow,
        )
        .await
    }
    async fn list_outbound(
        &self,
        source_short_id: ShortId,
        relation: &str,
        limit: u32,
        cursor_after: Option<ShortId>,
        narrow: &RowNarrow,
    ) -> Result<(Vec<SymbolRow>, u64), DbError> {
        Reader::list_outbound(
            &self.0,
            source_short_id,
            relation,
            limit,
            cursor_after,
            narrow,
        )
        .await
    }
    async fn list_module_files(
        &self,
        module_short_id: ShortId,
        limit: u32,
        cursor_after: Option<ShortId>,
    ) -> Result<(Vec<FileRow>, u64), DbError> {
        Reader::list_module_files(&self.0, module_short_id, limit, cursor_after).await
    }
    async fn find_symbol_tiered(
        &self,
        name: &str,
        limit: u32,
        include_external: bool,
        include_tests: bool,
    ) -> Result<Vec<FoundSymbolRow>, DbError> {
        Reader::find_symbol_tiered(&self.0, name, limit, include_external, include_tests).await
    }
    async fn search_symbols_by_name(
        &self,
        query: &str,
        limit: u32,
        cursor_score: Option<f32>,
        cursor_short_id: Option<ShortId>,
        include_external: bool,
        include_tests: bool,
    ) -> Result<(Vec<RankedSymbolRow>, u64), DbError> {
        Reader::search_symbols_by_name(
            &self.0,
            query,
            limit,
            cursor_score,
            cursor_short_id,
            include_external,
            include_tests,
        )
        .await
    }
    async fn search_symbols_blended(
        &self,
        query: &str,
        query_vec: Option<&[f32]>,
        target_total: u32,
        include_external: bool,
        include_tests: bool,
    ) -> Result<Vec<BlendedSymbolRow>, DbError> {
        Reader::search_symbols_blended(
            &self.0,
            query,
            query_vec,
            target_total,
            include_external,
            include_tests,
        )
        .await
    }
    async fn search_blended_hits(
        &self,
        query: &str,
        query_vec: Option<&[f32]>,
        target_total: u32,
        include_external: bool,
        include_tests: bool,
    ) -> Result<Vec<BlendedHit>, DbError> {
        Reader::search_blended_hits(
            &self.0,
            query,
            query_vec,
            target_total,
            include_external,
            include_tests,
        )
        .await
    }
    async fn scan_symbols(&self) -> Result<Vec<SymbolRow>, DbError> {
        Reader::scan_symbols(&self.0).await
    }
    async fn scan_files(&self) -> Result<Vec<FileRow>, DbError> {
        Reader::scan_files(&self.0).await
    }
    async fn scan_def_files(&self) -> Result<Vec<(ShortId, ShortId)>, DbError> {
        Reader::scan_def_files(&self.0).await
    }
    async fn scan_symbol_bodies(&self) -> Result<Vec<SymbolBodyRow>, DbError> {
        Reader::scan_symbol_bodies(&self.0).await
    }
    async fn scan_symbol_surfaces(&self, language: &str) -> Result<Vec<SymbolSurfaceRow>, DbError> {
        Reader::scan_symbol_surfaces(&self.0, language).await
    }
    async fn scan_file_docs(&self) -> Result<Vec<(ShortId, String)>, DbError> {
        Reader::scan_file_docs(&self.0).await
    }
    async fn scan_edges(&self, relation: &str) -> Result<Vec<(ShortId, ShortId)>, DbError> {
        Reader::scan_edges(&self.0, relation).await
    }
    async fn scan_aggregate_nodes(&self) -> Result<Vec<AggregateNodeRow>, DbError> {
        Reader::scan_aggregate_nodes(&self.0).await
    }
    async fn scan_aggregate_edges(&self) -> Result<Vec<AggregateEdgeRow>, DbError> {
        Reader::scan_aggregate_edges(&self.0).await
    }
    async fn scan_analysis_god_nodes(
        &self,
        filter: &str,
    ) -> Result<Vec<AnalysisGodNodeRow>, DbError> {
        Reader::scan_analysis_god_nodes(&self.0, filter).await
    }
    async fn scan_analysis_flat_communities(
        &self,
    ) -> Result<Vec<AnalysisFlatCommunityRow>, DbError> {
        Reader::scan_analysis_flat_communities(&self.0).await
    }
    async fn scan_analysis_anchored_hierarchy(
        &self,
    ) -> Result<Vec<AnalysisAnchoredCommunityRow>, DbError> {
        Reader::scan_analysis_anchored_hierarchy(&self.0).await
    }
    async fn scan_analysis_node_membership(
        &self,
    ) -> Result<Vec<AnalysisNodeMembershipRow>, DbError> {
        Reader::scan_analysis_node_membership(&self.0).await
    }
    async fn distinct_languages(&self) -> Result<Vec<String>, DbError> {
        Reader::distinct_languages(&self.0).await
    }
    async fn distinct_packages(&self) -> Result<Vec<String>, DbError> {
        Reader::distinct_packages(&self.0).await
    }
    async fn count_table(&self, table: &str) -> Result<u64, DbError> {
        Reader::count_table(&self.0, table).await
    }
}

/// Async write handle over one snapshot's three `SQLite` databases. `Clone`
/// is cheap (`Arc`) and all clones share one serialized `SqliteWriter` — the
/// indexer clones it across the batch sink and the aggregation pass.
#[derive(Clone)]
pub struct DbWriter {
    inner: Arc<Mutex<SqliteWriter>>,
    dir: Arc<PathBuf>,
}

impl DbWriter {
    /// Create the snapshot databases under `dir`.
    pub(crate) fn create(dir: &Path, options: WriterOptions) -> Result<Self, DbError> {
        Ok(Self {
            inner: Arc::new(Mutex::new(SqliteWriter::create(dir, options)?)),
            dir: Arc::new(dir.to_path_buf()),
        })
    }

    /// The run directory holding the snapshot databases.
    pub(crate) fn dir(&self) -> &Path {
        self.dir.as_path()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SqliteWriter> {
        self.inner.lock().expect("writer mutex poisoned")
    }

    #[expect(
        clippy::unused_async,
        reason = "async to keep a stable write API; delegates to the locked synchronous SqliteWriter"
    )]
    pub async fn write_batch(&self, batch: &WriteBatch) -> Result<(), DbError> {
        self.lock().write_batch(batch)
    }

    #[expect(
        clippy::unused_async,
        reason = "async to keep a stable write API; delegates to the locked synchronous SqliteWriter"
    )]
    pub async fn write_aggregate_tables(
        &self,
        nodes: &[AggregateNodeRecord],
        edges: &[AggregateEdgeRecord],
    ) -> Result<(), DbError> {
        self.lock().write_aggregate_tables(nodes, edges)
    }

    #[expect(
        clippy::unused_async,
        reason = "async to keep a stable write API; delegates to the locked synchronous SqliteWriter"
    )]
    pub async fn write_analysis_tables(
        &self,
        god_nodes: &[AnalysisGodNodeRecord],
        flat: &[AnalysisFlatCommunityRecord],
        anchored: &[AnalysisAnchoredCommunityRecord],
        membership: &[AnalysisNodeMembershipRecord],
    ) -> Result<(), DbError> {
        self.lock()
            .write_analysis_tables(god_nodes, flat, anchored, membership)
    }

    #[expect(
        clippy::unused_async,
        reason = "async to keep a stable write API; delegates to the locked synchronous SqliteWriter"
    )]
    pub async fn scan_symbols_for_aggregation(&self) -> Result<Vec<SymbolRecord>, DbError> {
        self.lock().scan_symbols_for_aggregation()
    }

    #[expect(
        clippy::unused_async,
        reason = "async to keep a stable write API; delegates to the locked synchronous SqliteWriter"
    )]
    pub async fn scan_files_for_aggregation(&self) -> Result<Vec<FileRecord>, DbError> {
        self.lock().scan_files_for_aggregation()
    }

    #[expect(
        clippy::unused_async,
        reason = "async to keep a stable write API; delegates to the locked synchronous SqliteWriter"
    )]
    pub async fn scan_packages_for_aggregation(&self) -> Result<Vec<PackageRecord>, DbError> {
        self.lock().scan_packages_for_aggregation()
    }

    #[expect(
        clippy::unused_async,
        reason = "async to keep a stable write API; delegates to the locked synchronous SqliteWriter"
    )]
    pub async fn scan_def_files_for_aggregation(
        &self,
    ) -> Result<Vec<(kenn_model::ShortId, kenn_model::ShortId)>, DbError> {
        self.lock().scan_def_files_for_aggregation()
    }

    #[expect(
        clippy::unused_async,
        reason = "async to keep a stable write API; delegates to the locked synchronous SqliteWriter"
    )]
    pub async fn scan_def_lines_for_aggregation(
        &self,
    ) -> Result<Vec<(kenn_model::ShortId, kenn_model::ShortId, u32, u32)>, DbError> {
        self.lock().scan_def_lines_for_aggregation()
    }

    #[expect(
        clippy::unused_async,
        reason = "async to keep a stable write API; delegates to the locked synchronous SqliteWriter"
    )]
    pub async fn scan_edges_for_aggregation(
        &self,
        kind: EdgeKind,
    ) -> Result<Vec<(kenn_model::ShortId, kenn_model::ShortId)>, DbError> {
        self.lock().scan_edges_for_aggregation(kind)
    }

    #[expect(
        clippy::unused_async,
        reason = "async to keep a stable write API; delegates to the locked synchronous SqliteWriter"
    )]
    pub async fn scan_analysis_flat_communities(
        &self,
    ) -> Result<Vec<AnalysisFlatCommunityRecord>, DbError> {
        self.lock().scan_analysis_flat_communities()
    }

    #[expect(
        clippy::unused_async,
        reason = "async to keep a stable write API; delegates to the locked synchronous SqliteWriter"
    )]
    pub async fn scan_analysis_node_membership(
        &self,
    ) -> Result<Vec<AnalysisNodeMembershipRecord>, DbError> {
        self.lock().scan_analysis_node_membership()
    }

    #[expect(
        clippy::unused_async,
        reason = "async to keep a stable write API; delegates to the locked synchronous SqliteWriter"
    )]
    pub async fn scan_file_docs(&self) -> Result<Vec<(kenn_model::ShortId, String)>, DbError> {
        self.lock().scan_file_docs()
    }

    #[expect(
        clippy::unused_async,
        reason = "async to keep a stable write API; delegates to the locked synchronous SqliteWriter"
    )]
    pub async fn write_stats(&self, rows: &[crate::api::types::StatRow]) -> Result<(), DbError> {
        self.lock().write_stats(rows)
    }

    #[expect(
        clippy::unused_async,
        reason = "async to keep a stable write API; delegates to the locked synchronous SqliteWriter"
    )]
    pub async fn finalize(&self) -> Result<(), DbError> {
        self.lock().finalize()
    }

    #[must_use]
    pub fn options(&self) -> WriterOptions {
        self.lock().options().clone()
    }
}
