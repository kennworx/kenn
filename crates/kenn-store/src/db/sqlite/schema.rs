//! DDL for the three per-snapshot `SQLite` databases (design D1, D2, D5).
//!
//! Column names and types mirror the retired Lance datasets 1:1 (arrow
//! `UInt*`→`INTEGER`, `Utf8`→`TEXT`, `Boolean`→`INTEGER`, `Float32`→`REAL`;
//! the `FixedSizeList<f32,768>` embedding becomes a `sqlite-vec` `vec0`
//! table). The engine enforces no intern uniqueness — `(name,version)` and
//! `(pub_id,pkg)` stay ingest-policy keys (index-store-db), so id columns
//! are indexed but not `UNIQUE` (per-language id counters collide).

use rusqlite::Connection;

/// `code.db` — the code graph (symbols/defs/edges/files/packages +
/// aggregate_* + analysis_*). Traversal reads these by bulk scan into the
/// in-memory CSR projection; point fetches use the indexed keys.
pub(crate) const GRAPH_DDL: &str = "
CREATE TABLE symbols (
  id INTEGER NOT NULL, pub_id TEXT NOT NULL, language TEXT NOT NULL, pkg_id INTEGER NOT NULL,
  kind TEXT NOT NULL, name TEXT NOT NULL, name_lower TEXT NOT NULL,
  enclosing_sym_id INTEGER NOT NULL, partial INTEGER NOT NULL, nargs INTEGER NOT NULL,
  targs INTEGER NOT NULL, external INTEGER NOT NULL, test INTEGER NOT NULL
);
CREATE INDEX symbols_id ON symbols(id);
CREATE INDEX symbols_lang_pub ON symbols(language, pub_id);
CREATE INDEX symbols_name_lower ON symbols(name_lower);

-- Separator-agnostic identifier index: each symbol's name split into lowercase
-- words (camelCase + snake_case → words), `unicode61`-tokenized, rowid-aligned
-- to `symbols.rowid`. Powers `find_symbol_tiered`'s word-split tier (design D6).
CREATE VIRTUAL TABLE name_words USING fts5(words, tokenize='unicode61');

CREATE TABLE symbol_docs (sym_id INTEGER NOT NULL, sig TEXT NOT NULL, doc TEXT NOT NULL);
CREATE INDEX symbol_docs_sym ON symbol_docs(sym_id);

CREATE TABLE file_docs (file_id INTEGER NOT NULL, doc TEXT NOT NULL);
CREATE INDEX file_docs_file ON file_docs(file_id);

CREATE TABLE defs (
  sym_id INTEGER NOT NULL, file_id INTEGER NOT NULL,
  start_line INTEGER NOT NULL, start_col INTEGER NOT NULL,
  end_line INTEGER NOT NULL, end_col INTEGER NOT NULL,
  body_start_line INTEGER NOT NULL DEFAULT 0, body_end_line INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX defs_sym ON defs(sym_id);
CREATE INDEX defs_file ON defs(file_id);

CREATE TABLE edges (
  src_id INTEGER NOT NULL, target_id INTEGER NOT NULL, kind INTEGER NOT NULL,
  field_op INTEGER, import_kind INTEGER, corr_source INTEGER,
  corr_generator TEXT, corr_canon_id INTEGER,
  link_grade INTEGER, link_relation TEXT
);
-- Traversal indexes: outbound by (src,kind) and inbound by (target,kind),
-- each carrying the far endpoint so `… AND other>? ORDER BY other` is covered.
CREATE INDEX edges_src ON edges(src_id, kind, target_id);
CREATE INDEX edges_tgt ON edges(target_id, kind, src_id);

CREATE TABLE files (
  id INTEGER NOT NULL, path TEXT NOT NULL, language TEXT NOT NULL,
  test INTEGER NOT NULL, external INTEGER NOT NULL, content_hash INTEGER NOT NULL
);
CREATE INDEX files_id ON files(id);
CREATE INDEX files_path ON files(path);

CREATE TABLE packages (
  id INTEGER NOT NULL, name TEXT NOT NULL, version TEXT NOT NULL,
  manager TEXT NOT NULL, external INTEGER NOT NULL
);
CREATE INDEX packages_id ON packages(id);

CREATE TABLE aggregate_nodes (
  id INTEGER NOT NULL, kind TEXT NOT NULL, name TEXT NOT NULL, language TEXT NOT NULL,
  external INTEGER NOT NULL, test INTEGER NOT NULL, example INTEGER NOT NULL,
  anchor_id INTEGER NOT NULL, anchor_name TEXT NOT NULL
);
CREATE TABLE aggregate_edges (
  src_id INTEGER NOT NULL, dst_id INTEGER NOT NULL, kind INTEGER NOT NULL, weight INTEGER NOT NULL
);

CREATE TABLE analysis_god_nodes (
  filter TEXT NOT NULL, rank INTEGER NOT NULL, short_id INTEGER NOT NULL,
  weighted_degree INTEGER NOT NULL, name TEXT NOT NULL, kind TEXT NOT NULL,
  anchor_id INTEGER NOT NULL, anchor_name TEXT NOT NULL
);
CREATE TABLE analysis_flat_communities (
  community_id INTEGER NOT NULL, size INTEGER NOT NULL, total_weight INTEGER NOT NULL,
  cross_anchor INTEGER NOT NULL, primary_anchor_id INTEGER NOT NULL, primary_anchor_name TEXT NOT NULL
);
CREATE TABLE analysis_anchored_hierarchy (
  community_id INTEGER NOT NULL, parent_id INTEGER NOT NULL, depth INTEGER NOT NULL,
  anchor_id INTEGER NOT NULL, anchor_name TEXT NOT NULL, size INTEGER NOT NULL,
  test_ratio REAL NOT NULL, test_infra INTEGER NOT NULL
);
CREATE TABLE analysis_node_membership (
  short_id INTEGER NOT NULL, flat_community_id INTEGER NOT NULL,
  anchored_leaf_community_id INTEGER NOT NULL
);

-- Build-time aggregate counts (build-time-stats). Narrow/long format: one row
-- per (scope, key, subset, metric). `subset` is the lens — internal|test|
-- external for entity metrics, 'graph' for clustering counters.
CREATE TABLE stats (
  scope TEXT NOT NULL, key TEXT NOT NULL, subset TEXT NOT NULL, metric TEXT NOT NULL,
  value INTEGER NOT NULL,
  PRIMARY KEY (scope, key, subset, metric)
);
";

/// `vector.db` — the code search store. One row per searchable unit
/// (`row_kind` name|doc), an FTS5 trigram index for identifier search, an
/// FTS5 stemming index for prose, and a `vec0` table for exact vector KNN.
/// FTS/vec rowids align with `knowledge.rowid`.
pub(crate) const KNOWLEDGE_DDL: &str = "
CREATE TABLE knowledge (
  rowid INTEGER PRIMARY KEY,
  embed_key TEXT NOT NULL, id INTEGER NOT NULL, row_kind TEXT NOT NULL, language TEXT NOT NULL,
  pub_id TEXT NOT NULL, path TEXT, name TEXT, kind TEXT, name_text TEXT, doc_text TEXT,
  fingerprint INTEGER NOT NULL
);
CREATE INDEX knowledge_id ON knowledge(id);
CREATE VIRTUAL TABLE name_fts USING fts5(name_text, tokenize='trigram');
CREATE VIRTUAL TABLE doc_fts  USING fts5(doc_text, tokenize='porter unicode61');
CREATE VIRTUAL TABLE vec_knowledge USING vec0(embedding float[768] distance_metric=cosine);
";

/// Apply the `code.db` DDL.
pub(crate) fn create_graph(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(GRAPH_DDL)
}

/// Apply the `vector.db` DDL (FTS5 + `vec0`).
pub(crate) fn create_knowledge(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(KNOWLEDGE_DDL)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn_with_vec() -> Connection {
        super::super::ensure_vec_extension();
        Connection::open_in_memory().expect("open in-memory")
    }

    fn table_exists(c: &Connection, name: &str) -> bool {
        let n: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [name],
                |r| r.get(0),
            )
            .unwrap();
        n == 1
    }

    #[test]
    fn graph_ddl_is_valid() {
        let c = Connection::open_in_memory().unwrap();
        create_graph(&c).expect("graph DDL executes");
        // Every table in the registry exists — registry and DDL cannot drift.
        for t in crate::db::names::code::ALL {
            assert!(table_exists(&c, t), "code.db table {t} missing");
        }
    }

    #[test]
    fn knowledge_ddl_creates_fts_and_vec0() {
        let c = conn_with_vec();
        create_knowledge(&c).expect("knowledge DDL executes (fts5 trigram + vec0)");
        // Every registry table exists (FTS5/vec0 virtual tables included).
        for t in crate::db::names::vector::ALL {
            assert!(table_exists(&c, t), "vector.db table {t} missing");
        }
        // vec0 KNN parses and runs against an empty table (validates the
        // `vec0`/`distance_metric=cosine` DDL and the MATCH query shape).
        let bytes: Vec<u8> = vec![0.0f32; 768]
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let mut stmt = c
            .prepare("SELECT rowid FROM vec_knowledge WHERE embedding MATCH ?1 ORDER BY distance LIMIT 5")
            .expect("vec0 KNN query prepares");
        let hits = stmt
            .query_map([bytes], |r| r.get::<_, i64>(0))
            .expect("vec0 KNN query runs")
            .count();
        assert_eq!(hits, 0);
    }
}
