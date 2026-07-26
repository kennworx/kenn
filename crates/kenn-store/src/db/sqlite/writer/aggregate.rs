//! Aggregate / analysis table writes + the aggregation-pass scans.

use rusqlite::params;

use kenn_model::{
    AggregateEdgeRecord, AggregateNodeRecord, AnalysisAnchoredCommunityRecord,
    AnalysisFlatCommunityRecord, AnalysisGodNodeRecord, AnalysisNodeMembershipRecord, EdgeKind,
    FileRecord, Kind, Language, PackageRecord, ShortId, SymbolRecord,
};

use crate::api::types::{DbError, StatRow};

use super::super::super::codes::edge_kind_code;
use super::core::{be, col_u16, col_u32, SqliteWriter};

impl SqliteWriter {
    /// Append the rolled-up `aggregate_nodes` / `aggregate_edges`.
    pub(crate) fn write_aggregate_tables(
        &self,
        nodes: &[AggregateNodeRecord],
        edges: &[AggregateEdgeRecord],
    ) -> Result<(), DbError> {
        let tx = self.graph.unchecked_transaction().map_err(be)?;
        {
            let mut ins = tx
                .prepare_cached(
                    "INSERT INTO aggregate_nodes(id,kind,name,language,external,test,example,\
                     anchor_id,anchor_name) VALUES(?,?,?,?,?,?,?,?,?)",
                )
                .map_err(be)?;
            for n in nodes {
                ins.execute(params![
                    n.id,
                    n.kind.db_name(),
                    n.name,
                    n.language.db_name(),
                    n.external,
                    n.test,
                    n.example,
                    n.anchor_id,
                    n.anchor_name,
                ])
                .map_err(be)?;
            }
        }
        {
            let mut ins = tx
                .prepare_cached(
                    "INSERT INTO aggregate_edges(src_id,dst_id,kind,weight) VALUES(?,?,?,?)",
                )
                .map_err(be)?;
            for e in edges {
                ins.execute(params![
                    e.src_id,
                    e.dst_id,
                    edge_kind_code(e.kind),
                    e.weight
                ])
                .map_err(be)?;
            }
        }
        tx.commit().map_err(be)?;
        Ok(())
    }

    /// Insert/replace build-time stat rows (build-time-stats). One surface for
    /// both producers — `finalize` (entity counts) and the analysis pass
    /// (graph counters) — keyed by `(scope, key, subset, metric)`.
    pub(crate) fn write_stats(&self, rows: &[StatRow]) -> Result<(), DbError> {
        let tx = self.graph.unchecked_transaction().map_err(be)?;
        {
            let mut ins = tx
                .prepare_cached(
                    "INSERT OR REPLACE INTO stats(scope,key,subset,metric,value) VALUES(?,?,?,?,?)",
                )
                .map_err(be)?;
            for r in rows {
                ins.execute(params![r.scope, r.key, r.subset, r.metric, r.value])
                    .map_err(be)?;
            }
        }
        tx.commit().map_err(be)?;
        Ok(())
    }

    /// Append the four persisted-analysis tables.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "non-negative weighted_degree / total_weight counts bound to a SQLite INTEGER parameter"
    )]
    pub(crate) fn write_analysis_tables(
        &self,
        god_nodes: &[AnalysisGodNodeRecord],
        flat: &[AnalysisFlatCommunityRecord],
        anchored: &[AnalysisAnchoredCommunityRecord],
        membership: &[AnalysisNodeMembershipRecord],
    ) -> Result<(), DbError> {
        let tx = self.graph.unchecked_transaction().map_err(be)?;
        {
            let mut ins = tx
                .prepare_cached(
                    "INSERT INTO analysis_god_nodes(filter,rank,short_id,weighted_degree,\
                     name,kind,anchor_id,anchor_name) VALUES(?,?,?,?,?,?,?,?)",
                )
                .map_err(be)?;
            for g in god_nodes {
                ins.execute(params![
                    g.filter.db_name(),
                    g.rank,
                    g.short_id,
                    g.weighted_degree as i64,
                    g.name,
                    g.kind.db_name(),
                    g.anchor_id,
                    g.anchor_name,
                ])
                .map_err(be)?;
            }
        }
        {
            let mut ins = tx
                .prepare_cached(
                    "INSERT INTO analysis_flat_communities(community_id,size,total_weight,\
                     cross_anchor,primary_anchor_id,primary_anchor_name) VALUES(?,?,?,?,?,?)",
                )
                .map_err(be)?;
            for f in flat {
                ins.execute(params![
                    f.community_id,
                    f.size,
                    f.total_weight as i64,
                    f.cross_anchor,
                    f.primary_anchor_id,
                    f.primary_anchor_name,
                ])
                .map_err(be)?;
            }
        }
        {
            let mut ins = tx
                .prepare_cached(
                    "INSERT INTO analysis_anchored_hierarchy(community_id,parent_id,depth,\
                     anchor_id,anchor_name,size,test_ratio,test_infra) VALUES(?,?,?,?,?,?,?,?)",
                )
                .map_err(be)?;
            for a in anchored {
                ins.execute(params![
                    a.community_id,
                    a.parent_id.unwrap_or(0),
                    a.depth,
                    a.anchor_id,
                    a.anchor_name,
                    a.size,
                    f64::from(a.test_ratio),
                    a.test_infra,
                ])
                .map_err(be)?;
            }
        }
        {
            let mut ins = tx
                .prepare_cached(
                    "INSERT INTO analysis_node_membership(short_id,flat_community_id,\
                     anchored_leaf_community_id) VALUES(?,?,?)",
                )
                .map_err(be)?;
            for m in membership {
                ins.execute(params![
                    m.short_id,
                    m.flat_community_id,
                    m.anchored_leaf_community_id
                ])
                .map_err(be)?;
            }
        }
        tx.commit().map_err(be)?;
        Ok(())
    }

    /// The persisted flat-Louvain communities, read back on the writer's own
    /// connection so the atlas (`domains` axis) can consume the analysis the
    /// post-aggregate hook just wrote — without opening a second reader or
    /// depending on `kenn-analyze`.
    #[expect(
        clippy::cast_sign_loss,
        reason = "non-negative total_weight stored as its i64 value; the i64 -> u64 cast round-trips"
    )]
    pub(crate) fn scan_analysis_flat_communities(
        &self,
    ) -> Result<Vec<AnalysisFlatCommunityRecord>, DbError> {
        let mut q = self
            .graph
            .prepare(
                "SELECT community_id,size,total_weight,cross_anchor,primary_anchor_id,\
                 primary_anchor_name FROM analysis_flat_communities",
            )
            .map_err(be)?;
        let rows = q
            .query_map([], |r| {
                Ok(AnalysisFlatCommunityRecord {
                    community_id: col_u32(r, 0)?,
                    size: col_u32(r, 1)?,
                    total_weight: r.get::<_, i64>(2)? as u64,
                    cross_anchor: r.get::<_, i64>(3)? != 0,
                    primary_anchor_id: col_u32(r, 4)?,
                    primary_anchor_name: r.get(5)?,
                })
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Per-aggregate-node community membership, read back for the atlas.
    pub(crate) fn scan_analysis_node_membership(
        &self,
    ) -> Result<Vec<AnalysisNodeMembershipRecord>, DbError> {
        let mut q = self
            .graph
            .prepare(
                "SELECT short_id,flat_community_id,anchored_leaf_community_id \
                 FROM analysis_node_membership",
            )
            .map_err(be)?;
        let rows = q
            .query_map([], |r| {
                Ok(AnalysisNodeMembershipRecord {
                    short_id: col_u32(r, 0)?,
                    flat_community_id: col_u32(r, 1)?,
                    anchored_leaf_community_id: col_u32(r, 2)?,
                })
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// File-level module docs (`file_docs` table), read back so the atlas can seed
    /// a package's `description` from its root-module doc. Empty docs are skipped.
    pub(crate) fn scan_file_docs(&self) -> Result<Vec<(ShortId, String)>, DbError> {
        let mut q = self
            .graph
            .prepare("SELECT file_id,doc FROM file_docs WHERE doc <> ''")
            .map_err(be)?;
        let rows = q
            .query_map([], |r| Ok((col_u32(r, 0)?, r.get::<_, String>(1)?)))
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Every symbol record, for the aggregation pass.
    pub(crate) fn scan_symbols_for_aggregation(&self) -> Result<Vec<SymbolRecord>, DbError> {
        let mut q = self
            .graph
            .prepare(
                "SELECT id,pub_id,language,pkg_id,kind,name,enclosing_sym_id,partial,\
                 nargs,targs,external,test FROM symbols",
            )
            .map_err(be)?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    col_u32(r, 0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    col_u32(r, 3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    col_u32(r, 6)?,
                    r.get::<_, i64>(7)? != 0,
                    col_u16(r, 8)?,
                    col_u16(r, 9)?,
                    r.get::<_, i64>(10)? != 0,
                    r.get::<_, i64>(11)? != 0,
                ))
            })
            .map_err(be)?;
        let mut out = Vec::new();
        for row in rows {
            let (id, pub_id, lang, pkg_id, kind, name, enc, partial, nargs, targs, external, test) =
                row.map_err(be)?;
            out.push(SymbolRecord {
                id,
                pub_id,
                language: Language::from_db_name(&lang)
                    .ok_or_else(|| DbError::Backend(format!("bad language `{lang}`")))?,
                pkg_id,
                kind: Kind::from_db_name(&kind)
                    .ok_or_else(|| DbError::Backend(format!("bad kind `{kind}`")))?,
                name,
                enclosing_sym_id: enc,
                partial,
                nargs,
                targs,
                external,
                test,
            });
        }
        Ok(out)
    }

    /// Every file record, for the aggregation pass.
    #[expect(
        clippy::cast_sign_loss,
        reason = "u64 content_hash stored as its i64 bit pattern; the i64 -> u64 cast round-trips losslessly"
    )]
    pub(crate) fn scan_files_for_aggregation(&self) -> Result<Vec<FileRecord>, DbError> {
        let mut q = self
            .graph
            .prepare("SELECT id,path,language,test,external,content_hash FROM files")
            .map_err(be)?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    col_u32(r, 0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)? != 0,
                    r.get::<_, i64>(4)? != 0,
                    r.get::<_, i64>(5)? as u64,
                ))
            })
            .map_err(be)?;
        let mut out = Vec::new();
        for row in rows {
            let (id, path, lang, test, external, content_hash) = row.map_err(be)?;
            out.push(FileRecord {
                id,
                path,
                language: Language::from_db_name(&lang)
                    .ok_or_else(|| DbError::Backend(format!("bad language `{lang}`")))?,
                test,
                external,
                content_hash,
            });
        }
        Ok(out)
    }

    /// Every package record, for the aggregation pass.
    pub(crate) fn scan_packages_for_aggregation(&self) -> Result<Vec<PackageRecord>, DbError> {
        let mut q = self
            .graph
            .prepare("SELECT id,name,version,manager,external FROM packages")
            .map_err(be)?;
        let rows = q
            .query_map([], |r| {
                Ok(PackageRecord {
                    id: col_u32(r, 0)?,
                    name: r.get(1)?,
                    version: r.get(2)?,
                    manager: r.get(3)?,
                    external: r.get::<_, i64>(4)? != 0,
                })
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Every `(sym_id, file_id)` declaration pair, for the aggregation pass.
    pub(crate) fn scan_def_files_for_aggregation(&self) -> Result<Vec<(u32, u32)>, DbError> {
        let mut q = self
            .graph
            .prepare("SELECT DISTINCT sym_id, file_id FROM defs")
            .map_err(be)?;
        let rows = q
            .query_map([], |r| Ok((col_u32(r, 0)?, col_u32(r, 1)?)))
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Every `(sym_id, file_id, start_line, end_line)` declaration row, for the
    /// atlas producer's per-symbol source location. The range is the **full item
    /// extent** — `body_start_line..body_end_line` (the enclosing range, which
    /// spans the whole def) when present, else the name-token `start..end` line.
    pub(crate) fn scan_def_lines_for_aggregation(
        &self,
    ) -> Result<Vec<(u32, u32, u32, u32)>, DbError> {
        let mut q = self
            .graph
            .prepare(
                "SELECT sym_id, file_id, \
                 CASE WHEN body_end_line > 0 THEN body_start_line ELSE start_line END, \
                 CASE WHEN body_end_line > 0 THEN body_end_line ELSE end_line END \
                 FROM defs",
            )
            .map_err(be)?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    col_u32(r, 0)?,
                    col_u32(r, 1)?,
                    col_u32(r, 2)?,
                    col_u32(r, 3)?,
                ))
            })
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Every `(source, target)` pair of one edge kind, for the aggregation pass.
    pub(crate) fn scan_edges_for_aggregation(
        &self,
        kind: EdgeKind,
    ) -> Result<Vec<(u32, u32)>, DbError> {
        let code = edge_kind_code(kind);
        let mut q = self
            .graph
            .prepare("SELECT DISTINCT src_id, target_id FROM edges WHERE kind = ?1")
            .map_err(be)?;
        let rows = q
            .query_map([code], |r| Ok((col_u32(r, 0)?, col_u32(r, 1)?)))
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }
}
