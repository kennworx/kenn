//! The async [`Reader`] surface over a snapshot's connection [`Pool`].
//!
//! Every method runs a synchronous [`SqliteConnRef`] core on a pooled
//! background-thread connection (off the runtime workers; N connections serve N
//! concurrent reads). The [`SqliteReader::with_conn`] helper hides the
//! `Pool::conn_and_then` + `SqliteConnRef` wiring, so each method body is just
//! the closure calling its core. Borrowed arguments are cloned into the
//! `'static` closure; everything else is the same query as before.
//!
//! [`Pool`]: async_sqlite::Pool

use super::projection::{SqliteConnRef, SqliteReader};
use crate::api::types::{
    AggregateEdgeRow, AggregateNodeRow, AnalysisAnchoredCommunityRow, AnalysisFlatCommunityRow,
    AnalysisGodNodeRow, AnalysisNodeMembershipRow, BlendedHit, BlendedSymbolRow, DbError,
    DefLineRow, DefRow, FileRow, FoundSymbolRow, PackageRow, RankedSymbolRow, StatRow,
    SymbolBodyRow, SymbolDocsRow, SymbolRow, SymbolSurfaceRow,
};
use crate::api::{Reader, RowNarrow};
use kenn_model::ShortId;

impl SqliteReader {
    /// Run a synchronous [`SqliteConnRef`] core on a pooled connection. Hides
    /// the `Pool::conn_and_then` + `SqliteConnRef { conn }` boilerplate so each
    /// caller's closure is just `|c| c.some_method(args)`.
    pub(super) async fn with_conn<F, T>(&self, f: F) -> Result<T, DbError>
    where
        F: for<'a> FnOnce(SqliteConnRef<'a>) -> Result<T, DbError> + Send + 'static,
        T: Send + 'static,
    {
        self.pool
            .conn_and_then(move |conn| f(SqliteConnRef { conn }))
            .await
    }

    /// Item-to-item similar symbols (the `find_similar` MCP tool). `None` when
    /// the source symbol has no committed embedding (vectors not built).
    pub(crate) async fn find_similar_symbols(
        &self,
        source: ShortId,
        limit: u32,
        include_external: bool,
        include_tests: bool,
    ) -> Result<Option<Vec<RankedSymbolRow>>, DbError> {
        self.with_conn(move |c| {
            c.find_similar_symbols(source, limit, include_external, include_tests)
        })
        .await
    }

    /// Code FILE rows whose basename equals `basename` (md→code resolution).
    pub(crate) async fn files_by_basename(&self, basename: &str) -> Result<Vec<FileRow>, DbError> {
        let basename = basename.to_owned();
        self.with_conn(move |c| c.files_by_basename(&basename))
            .await
    }

    /// Code SYMBOL rows whose short name equals `name` (md→code resolution).
    pub(crate) async fn symbols_by_short_name(
        &self,
        name: &str,
    ) -> Result<Vec<crate::api::types::CodeSymbolHit>, DbError> {
        let name = name.to_owned();
        self.with_conn(move |c| c.symbols_by_short_name(&name))
            .await
    }

    /// Non-exact markdown links for `check_links` (the link-grade read path),
    /// optionally filtered to `grade_codes`, capped at `limit` rows, with the
    /// full matching count.
    pub(crate) async fn scan_link_diagnostics(
        &self,
        grade_codes: Option<Vec<u8>>,
        limit: u32,
    ) -> Result<(Vec<crate::api::types::LinkDiagnosticRow>, u64), DbError> {
        self.with_conn(move |c| c.scan_link_diagnostics(grade_codes.as_deref(), limit))
            .await
    }

    /// Dead-CSS findings for `check_css` (orphan classes / stylesheets), bounded
    /// at `limit` rows with the full per-category counts.
    pub(crate) async fn scan_css_health(
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
        self.with_conn(move |c| c.scan_css_health(want_classes, want_sheets, limit))
            .await
    }

    /// A resolver over this snapshot's code-node ids, for findings staleness.
    pub(crate) async fn code_node_resolver(
        &self,
    ) -> Result<super::super::super::findings::CodeGraphNodeResolver, DbError> {
        self.with_conn(|c| c.code_node_resolver()).await
    }

    /// All build-time `stats` rows.
    pub(crate) async fn stats(&self) -> Result<Vec<StatRow>, DbError> {
        self.with_conn(|c| c.stats()).await
    }
}

impl Reader for SqliteReader {
    async fn fetch_symbol_pub_id(&self, short_id: ShortId) -> Result<Option<String>, DbError> {
        self.with_conn(move |c| c.fetch_symbol_pub_id(short_id))
            .await
    }
    async fn fetch_symbol(
        &self,
        language: &str,
        pub_id: &str,
    ) -> Result<Option<SymbolRow>, DbError> {
        let (language, pub_id) = (language.to_owned(), pub_id.to_owned());
        self.with_conn(move |c| c.fetch_symbol(&language, &pub_id))
            .await
    }
    async fn fetch_symbol_by_short_id(
        &self,
        short_id: ShortId,
    ) -> Result<Option<SymbolRow>, DbError> {
        self.with_conn(move |c| c.fetch_symbol_by_short_id(short_id))
            .await
    }
    async fn fetch_symbol_docs_row(
        &self,
        symbol_short_id: ShortId,
    ) -> Result<Option<SymbolDocsRow>, DbError> {
        self.with_conn(move |c| c.fetch_symbol_docs_row(symbol_short_id))
            .await
    }
    async fn fetch_defs(&self, symbol_short_id: ShortId) -> Result<Vec<DefRow>, DbError> {
        self.with_conn(move |c| c.fetch_defs(symbol_short_id)).await
    }
    async fn fetch_def_lines(&self, symbol_short_id: ShortId) -> Result<Vec<DefLineRow>, DbError> {
        self.with_conn(move |c| c.fetch_def_lines(symbol_short_id))
            .await
    }
    async fn fetch_package(&self, short_id: ShortId) -> Result<Option<PackageRow>, DbError> {
        self.with_conn(move |c| c.fetch_package(short_id)).await
    }
    async fn fetch_file_path(&self, short_id: ShortId) -> Result<Option<String>, DbError> {
        self.with_conn(move |c| c.fetch_file_path(short_id)).await
    }
    async fn fetch_file_short_id(&self, path: &str) -> Result<Option<ShortId>, DbError> {
        let path = path.to_owned();
        self.with_conn(move |c| c.fetch_file_short_id(&path)).await
    }
    async fn find_at_location(
        &self,
        file_short_id: ShortId,
        line: u32,
    ) -> Result<Vec<SymbolRow>, DbError> {
        self.with_conn(move |c| c.find_at_location(file_short_id, line))
            .await
    }
    async fn list_inbound(
        &self,
        target_short_id: ShortId,
        relation: &str,
        limit: u32,
        cursor_after: Option<ShortId>,
        narrow: &RowNarrow,
    ) -> Result<(Vec<SymbolRow>, u64), DbError> {
        let relation = relation.to_owned();
        let narrow = narrow.clone();
        self.with_conn(move |c| {
            c.list_inbound(target_short_id, &relation, limit, cursor_after, &narrow)
        })
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
        let relation = relation.to_owned();
        let narrow = narrow.clone();
        self.with_conn(move |c| {
            c.list_outbound(source_short_id, &relation, limit, cursor_after, &narrow)
        })
        .await
    }
    async fn list_module_files(
        &self,
        module_short_id: ShortId,
        limit: u32,
        cursor_after: Option<ShortId>,
    ) -> Result<(Vec<FileRow>, u64), DbError> {
        self.with_conn(move |c| c.list_module_files(module_short_id, limit, cursor_after))
            .await
    }
    async fn find_symbol_tiered(
        &self,
        name: &str,
        limit: u32,
        include_external: bool,
        include_tests: bool,
    ) -> Result<Vec<FoundSymbolRow>, DbError> {
        let name = name.to_owned();
        self.with_conn(move |c| c.find_symbol_tiered(&name, limit, include_external, include_tests))
            .await
    }
    async fn search_symbols_by_name(
        &self,
        query: &str,
        limit: u32,
        _cursor_score: Option<f32>,
        _cursor_short_id: Option<ShortId>,
        include_external: bool,
        include_tests: bool,
    ) -> Result<(Vec<RankedSymbolRow>, u64), DbError> {
        let query = query.to_owned();
        self.with_conn(move |c| {
            let v = c.search_symbols_by_name(&query, limit, include_external, include_tests)?;
            let total = v.len() as u64;
            Ok((v, total))
        })
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
        let query = query.to_owned();
        let query_vec = query_vec.map(<[f32]>::to_vec);
        self.with_conn(move |c| {
            c.search_symbols_blended(
                &query,
                query_vec.as_deref(),
                target_total,
                include_external,
                include_tests,
            )
        })
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
        let query = query.to_owned();
        let query_vec = query_vec.map(<[f32]>::to_vec);
        self.with_conn(move |c| {
            c.search_blended_hits(
                &query,
                query_vec.as_deref(),
                target_total,
                include_external,
                include_tests,
            )
        })
        .await
    }
    async fn scan_symbols(&self) -> Result<Vec<SymbolRow>, DbError> {
        self.with_conn(|c| c.scan_symbols()).await
    }
    async fn scan_files(&self) -> Result<Vec<FileRow>, DbError> {
        self.with_conn(|c| c.scan_files()).await
    }
    async fn scan_def_files(&self) -> Result<Vec<(ShortId, ShortId)>, DbError> {
        self.with_conn(|c| c.scan_def_files()).await
    }
    async fn scan_symbol_bodies(&self) -> Result<Vec<SymbolBodyRow>, DbError> {
        self.with_conn(|c| c.scan_symbol_bodies()).await
    }
    async fn scan_symbol_surfaces(&self, language: &str) -> Result<Vec<SymbolSurfaceRow>, DbError> {
        let language = language.to_owned();
        self.with_conn(move |c| c.scan_symbol_surfaces(&language))
            .await
    }
    async fn scan_file_docs(&self) -> Result<Vec<(ShortId, String)>, DbError> {
        self.with_conn(|c| c.scan_file_docs()).await
    }
    async fn scan_edges(&self, relation: &str) -> Result<Vec<(ShortId, ShortId)>, DbError> {
        let relation = relation.to_owned();
        self.with_conn(move |c| c.scan_edges(&relation)).await
    }
    async fn scan_aggregate_nodes(&self) -> Result<Vec<AggregateNodeRow>, DbError> {
        self.with_conn(|c| c.scan_aggregate_nodes()).await
    }
    async fn scan_aggregate_edges(&self) -> Result<Vec<AggregateEdgeRow>, DbError> {
        self.with_conn(|c| c.scan_aggregate_edges()).await
    }
    async fn scan_analysis_god_nodes(
        &self,
        filter: &str,
    ) -> Result<Vec<AnalysisGodNodeRow>, DbError> {
        let filter = filter.to_owned();
        self.with_conn(move |c| c.scan_analysis_god_nodes(&filter))
            .await
    }
    async fn scan_analysis_flat_communities(
        &self,
    ) -> Result<Vec<AnalysisFlatCommunityRow>, DbError> {
        self.with_conn(|c| c.scan_analysis_flat_communities()).await
    }
    async fn scan_analysis_anchored_hierarchy(
        &self,
    ) -> Result<Vec<AnalysisAnchoredCommunityRow>, DbError> {
        self.with_conn(|c| c.scan_analysis_anchored_hierarchy())
            .await
    }
    async fn scan_analysis_node_membership(
        &self,
    ) -> Result<Vec<AnalysisNodeMembershipRow>, DbError> {
        self.with_conn(|c| c.scan_analysis_node_membership()).await
    }
    async fn distinct_languages(&self) -> Result<Vec<String>, DbError> {
        self.with_conn(|c| c.distinct_languages()).await
    }
    async fn distinct_packages(&self) -> Result<Vec<String>, DbError> {
        self.with_conn(|c| c.distinct_packages()).await
    }
    async fn count_table(&self, table: &str) -> Result<u64, DbError> {
        let table = table.to_owned();
        self.with_conn(move |c| c.count_table(&table)).await
    }
}
