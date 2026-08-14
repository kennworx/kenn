//! Whole-graph scan + aggregate/analysis inherent methods on [`SqliteConn`].

use super::super::super::codes::{edge_kind_code, edge_kind_from_code, parse_edge_relation};
use super::projection::{
    be, col_f32, col_u32, col_u64, file_from_row, symbol_from_row, SqliteConnRef, SYMBOL_COLS,
};
use crate::api::types::{
    AggregateEdgeRow, AggregateNodeRow, AnalysisAnchoredCommunityRow, AnalysisFlatCommunityRow,
    AnalysisGodNodeRow, AnalysisNodeMembershipRow, DbError, FileRow, SymbolBodyRow, SymbolRow,
    SymbolSurfaceRow,
};

impl SqliteConnRef<'_> {
    /// Every symbol row (whole-graph analysis; not on the MCP hot path).
    pub(crate) fn scan_symbols(&self) -> Result<Vec<SymbolRow>, DbError> {
        let mut q = self
            .conn
            .prepare(&format!("SELECT {SYMBOL_COLS} FROM symbols"))
            .map_err(be)?;
        let rows = q.query_map([], symbol_from_row).map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Every file row (whole-workspace shape questions; not on the hot path).
    pub(crate) fn scan_files(&self) -> Result<Vec<FileRow>, DbError> {
        let mut q = self
            .conn
            .prepare("SELECT id, path, language, test, external FROM files ORDER BY id")
            .map_err(be)?;
        let rows = q.query_map([], file_from_row).map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Every `(sym_id, file_id)` definition pair. Used for whole-workspace
    /// shape questions (which files a package's symbols are defined in); not on
    /// the MCP hot path.
    pub(crate) fn scan_def_files(&self) -> Result<Vec<(u32, u32)>, DbError> {
        let mut q = self
            .conn
            .prepare("SELECT DISTINCT sym_id, file_id FROM defs")
            .map_err(be)?;
        let rows = q
            .query_map([], |r| Ok((col_u32(r, 0)?, col_u32(r, 1)?)))
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Every symbol carrying a usable enclosing-item extent, with its file.
    ///
    /// Only rows with a real span are returned: `body_start_line >= 1` and an
    /// end at or after it. A zero extent means the producer captured none —
    /// there is nothing to read back — and returning it would make every
    /// consumer re-filter.
    pub(crate) fn scan_symbol_bodies(&self) -> Result<Vec<SymbolBodyRow>, DbError> {
        let mut q = self
            .conn
            .prepare(
                "SELECT d.sym_id, f.path, f.language, d.body_start_line, d.body_end_line, s.test \
                 FROM defs d \
                 JOIN files f ON f.id = d.file_id \
                 JOIN symbols s ON s.id = d.sym_id \
                 WHERE d.body_start_line >= 1 AND d.body_end_line >= d.body_start_line",
            )
            .map_err(be)?;
        let rows = q
            .query_map([], |r| {
                Ok(SymbolBodyRow {
                    sym_id: col_u32(r, 0)?,
                    path: r.get::<_, String>(1)?,
                    language: r.get::<_, String>(2)?,
                    body_start_line: col_u32(r, 3)?,
                    body_end_line: col_u32(r, 4)?,
                    test: r.get::<_, i64>(5)? != 0,
                })
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Every symbol of one language with its stored signature and content.
    ///
    /// The path rides along so a caller can confine itself to configured roots
    /// without a second query per symbol.
    pub(crate) fn scan_symbol_surfaces(
        &self,
        language: &str,
    ) -> Result<Vec<SymbolSurfaceRow>, DbError> {
        let mut q = self
            .conn
            .prepare(
                "SELECT s.id, s.pub_id, COALESCE(f.path, ''), \
                 COALESCE(sd.sig, ''), COALESCE(sd.doc, '') \
                 FROM symbols s \
                 LEFT JOIN symbol_docs sd ON sd.sym_id = s.id \
                 LEFT JOIN defs d ON d.sym_id = s.id \
                 LEFT JOIN files f ON f.id = d.file_id \
                 WHERE s.language = ?1",
            )
            .map_err(be)?;
        let rows = q
            .query_map([language], |r| {
                Ok(SymbolSurfaceRow {
                    sym_id: col_u32(r, 0)?,
                    pub_id: r.get::<_, String>(1)?,
                    path: r.get::<_, String>(2)?,
                    sig: r.get::<_, String>(3)?,
                    doc: r.get::<_, String>(4)?,
                })
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Every non-empty `(file_id, doc)` module doc.
    pub(crate) fn scan_file_docs(&self) -> Result<Vec<(u32, String)>, DbError> {
        let mut q = self
            .conn
            .prepare("SELECT file_id, doc FROM file_docs WHERE doc <> ''")
            .map_err(be)?;
        let rows = q
            .query_map([], |r| Ok((col_u32(r, 0)?, r.get::<_, String>(1)?)))
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Every `(source, target)` pair for `relation`, deduped and sorted.
    pub(crate) fn scan_edges(&self, relation: &str) -> Result<Vec<(u32, u32)>, DbError> {
        let Some(kind) = parse_edge_relation(relation) else {
            return Err(DbError::Backend(format!("unknown relation: {relation}")));
        };
        let want = i64::from(edge_kind_code(kind));
        let mut q = self
            .conn
            .prepare_cached(
                "SELECT DISTINCT src_id, target_id FROM edges WHERE kind=?1 \
                 ORDER BY src_id, target_id",
            )
            .map_err(be)?;
        let rows = q
            .query_map([want], |r| Ok((col_u32(r, 0)?, col_u32(r, 1)?)))
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    // ── aggregate / analysis scans (cold path, live over code.db) ──────

    /// The rolled-up module-level `aggregate_nodes`.
    pub(crate) fn scan_aggregate_nodes(&self) -> Result<Vec<AggregateNodeRow>, DbError> {
        let mut q = self
            .conn
            .prepare(
                "SELECT id,kind,name,language,external,test,example,anchor_id,anchor_name \
                 FROM aggregate_nodes",
            )
            .map_err(be)?;
        let rows = q
            .query_map([], |r| {
                Ok(AggregateNodeRow {
                    id: col_u32(r, 0)?,
                    kind: r.get(1)?,
                    name: r.get(2)?,
                    language: r.get(3)?,
                    external: r.get::<_, i64>(4)? != 0,
                    test: r.get::<_, i64>(5)? != 0,
                    example: r.get::<_, i64>(6)? != 0,
                    anchor_id: col_u32(r, 7)?,
                    anchor_name: r.get(8)?,
                })
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// The deduped undirected `aggregate_edges`.
    pub(crate) fn scan_aggregate_edges(&self) -> Result<Vec<AggregateEdgeRow>, DbError> {
        let mut q = self
            .conn
            .prepare("SELECT src_id,dst_id,kind,weight FROM aggregate_edges")
            .map_err(be)?;
        let rows = q
            .query_map([], |r| {
                Ok(AggregateEdgeRow {
                    src_id: col_u32(r, 0)?,
                    dst_id: col_u32(r, 1)?,
                    kind: edge_kind_from_code(col_u32(r, 2)?),
                    weight: col_u32(r, 3)?,
                })
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// The `analysis_god_nodes` for one centrality `filter`, rank-ordered.
    pub(crate) fn scan_analysis_god_nodes(
        &self,
        filter: &str,
    ) -> Result<Vec<AnalysisGodNodeRow>, DbError> {
        let mut q = self
            .conn
            .prepare(
                "SELECT filter,rank,short_id,weighted_degree,name,kind,anchor_id,anchor_name \
                 FROM analysis_god_nodes WHERE filter = ?1 ORDER BY rank",
            )
            .map_err(be)?;
        let rows = q
            .query_map([filter], |r| {
                Ok(AnalysisGodNodeRow {
                    filter: r.get(0)?,
                    rank: col_u32(r, 1)?,
                    short_id: col_u32(r, 2)?,
                    weighted_degree: col_u64(r, 3)?,
                    name: r.get(4)?,
                    kind: r.get(5)?,
                    anchor_id: col_u32(r, 6)?,
                    anchor_name: r.get(7)?,
                })
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// The flat (non-hierarchical) Louvain communities.
    pub(crate) fn scan_analysis_flat_communities(
        &self,
    ) -> Result<Vec<AnalysisFlatCommunityRow>, DbError> {
        let mut q = self
            .conn
            .prepare(
                "SELECT community_id,size,total_weight,cross_anchor,primary_anchor_id,\
                 primary_anchor_name FROM analysis_flat_communities",
            )
            .map_err(be)?;
        let rows = q
            .query_map([], |r| {
                Ok(AnalysisFlatCommunityRow {
                    community_id: col_u32(r, 0)?,
                    size: col_u32(r, 1)?,
                    total_weight: col_u64(r, 2)?,
                    cross_anchor: r.get::<_, i64>(3)? != 0,
                    primary_anchor_id: col_u32(r, 4)?,
                    primary_anchor_name: r.get(5)?,
                })
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// The recursive anchored-Louvain hierarchy, one row per tree node.
    pub(crate) fn scan_analysis_anchored_hierarchy(
        &self,
    ) -> Result<Vec<AnalysisAnchoredCommunityRow>, DbError> {
        let mut q = self
            .conn
            .prepare(
                "SELECT community_id,parent_id,depth,anchor_id,anchor_name,size,test_ratio,\
                 test_infra FROM analysis_anchored_hierarchy",
            )
            .map_err(be)?;
        let rows = q
            .query_map([], |r| {
                Ok(AnalysisAnchoredCommunityRow {
                    community_id: col_u32(r, 0)?,
                    parent_id: col_u32(r, 1)?,
                    depth: col_u32(r, 2)?,
                    anchor_id: col_u32(r, 3)?,
                    anchor_name: r.get(4)?,
                    size: col_u32(r, 5)?,
                    test_ratio: col_f32(r, 6)?,
                    test_infra: r.get::<_, i64>(7)? != 0,
                })
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Each symbol's flat + anchored-leaf community membership.
    pub(crate) fn scan_analysis_node_membership(
        &self,
    ) -> Result<Vec<AnalysisNodeMembershipRow>, DbError> {
        let mut q = self
            .conn
            .prepare(
                "SELECT short_id,flat_community_id,anchored_leaf_community_id \
                 FROM analysis_node_membership",
            )
            .map_err(be)?;
        let rows = q
            .query_map([], |r| {
                Ok(AnalysisNodeMembershipRow {
                    short_id: col_u32(r, 0)?,
                    flat_community_id: col_u32(r, 1)?,
                    anchored_leaf_community_id: col_u32(r, 2)?,
                })
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }
}
