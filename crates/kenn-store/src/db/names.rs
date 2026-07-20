//! Central registry of on-disk database file names and the SQL table names
//! within them — the single place to review or rename them.
//!
//! The DDL that creates these tables lives in [`super::sqlite::schema`]; the
//! `schema_matches_table_registry` test there asserts every name below actually
//! exists in the built schema, so this registry and the DDL cannot drift.

/// Snapshot databases, per index run (under the run dir / live snapshot).
pub(crate) const CODE_DB: &str = "code.db";
pub(crate) const VECTOR_DB: &str = "vector.db";

/// The findings index — a derived `SQLite` DB at the derived root, with a
/// lifetime independent of the per-run snapshots.
pub(crate) const FINDINGS_DB: &str = "findings.db";

/// `code.db` tables — the structural code graph.
///
/// `dead_code` is allowed: this is a human-facing name registry. The DDL and
/// queries spell most table names as SQL literals (for readability and
/// greppability), so only a subset is referenced from Rust; the rest exist
/// here as the single review point, kept honest by the `graph_ddl_is_valid`
/// drift test. The drift test (test build) references them all; the lib build
/// does not, hence the `not(test)` expectation.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "name registry; SQL uses literals, drift test pins it"
    )
)]
pub(crate) mod code {
    pub(crate) const SYMBOLS: &str = "symbols";
    pub(crate) const NAME_WORDS: &str = "name_words";
    pub(crate) const SYMBOL_DOCS: &str = "symbol_docs";
    pub(crate) const FILE_DOCS: &str = "file_docs";
    pub(crate) const DEFS: &str = "defs";
    pub(crate) const EDGES: &str = "edges";
    pub(crate) const FILES: &str = "files";
    pub(crate) const PACKAGES: &str = "packages";
    pub(crate) const AGGREGATE_NODES: &str = "aggregate_nodes";
    pub(crate) const AGGREGATE_EDGES: &str = "aggregate_edges";
    pub(crate) const ANALYSIS_GOD_NODES: &str = "analysis_god_nodes";
    pub(crate) const ANALYSIS_FLAT_COMMUNITIES: &str = "analysis_flat_communities";
    pub(crate) const ANALYSIS_ANCHORED_HIERARCHY: &str = "analysis_anchored_hierarchy";
    pub(crate) const ANALYSIS_NODE_MEMBERSHIP: &str = "analysis_node_membership";
    pub(crate) const STATS: &str = "stats";

    /// Every `code.db` table, for the schema-drift test.
    pub(crate) const ALL: &[&str] = &[
        SYMBOLS,
        NAME_WORDS,
        SYMBOL_DOCS,
        FILE_DOCS,
        DEFS,
        EDGES,
        FILES,
        PACKAGES,
        AGGREGATE_NODES,
        AGGREGATE_EDGES,
        ANALYSIS_GOD_NODES,
        ANALYSIS_FLAT_COMMUNITIES,
        ANALYSIS_ANCHORED_HIERARCHY,
        ANALYSIS_NODE_MEMBERSHIP,
        STATS,
    ];
}

/// `vector.db` tables — the code search store (FTS5 + `vec0`).
///
/// `dead_code` allowed for the same reason as [`code`]: a name registry whose
/// members are mostly spelled as SQL literals; the `knowledge_ddl_creates_fts_and_vec0`
/// drift test asserts they match the DDL.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "name registry; SQL uses literals, drift test pins it"
    )
)]
pub(crate) mod vector {
    pub(crate) const KNOWLEDGE: &str = "knowledge";
    pub(crate) const NAME_FTS: &str = "name_fts";
    pub(crate) const DOC_FTS: &str = "doc_fts";
    pub(crate) const VEC_KNOWLEDGE: &str = "vec_knowledge";

    /// Every `vector.db` table, for the schema-drift test.
    pub(crate) const ALL: &[&str] = &[KNOWLEDGE, NAME_FTS, DOC_FTS, VEC_KNOWLEDGE];
}
