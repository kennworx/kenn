//! `finalize`: derive the knowledge search rows from the populated graph
//! tables and feed FTS5 / `vec0`.

use std::collections::HashMap;

use rusqlite::{params, Transaction};

use crate::api::types::{DbError, StatRow};
use crate::embed::sidecar::QuantVector;

use super::core::{be, SqliteWriter};

impl SqliteWriter {
    /// Derive the knowledge search rows from the populated graph tables and
    /// feed FTS5. One name row per symbol, one doc row per non-empty
    /// `symbol_docs` / `file_docs` entry; committed sidecar vectors are
    /// reconciled into `vec0` by fingerprint (design D5), and the background
    /// embed job fills the rest. The three passes are extracted into
    /// [`Self::build_name_rows`] etc.
    pub(crate) fn finalize(&self) -> Result<(), DbError> {
        // Drop spurious zero-range placeholder defs. The per-document SCIP /
        // JSONL transform emits a `[0,0,0,0]` placeholder for every symbol in a
        // document's symbol table that lacks a *local* Definition occurrence —
        // but that symbol may be defined in another document, so once the whole
        // graph is ingested those placeholders are redundant duplicates of the
        // real def. Keep a placeholder only when it is a symbol's ONLY def
        // (truly synthetic / external, so the symbol stays addressable). Runs
        // before `build_entity_stats` so the def counts don't include them.
        self.graph
            .execute(
                "DELETE FROM defs WHERE start_line = 0 \
                 AND sym_id IN (SELECT sym_id FROM defs WHERE start_line >= 1)",
                [],
            )
            .map_err(be)?;
        let ktx = self.knowledge.unchecked_transaction().map_err(be)?;
        // Reuse committed sidecar vectors (fingerprint → int8) into `vec0` — no
        // embedding model needed; the background embed job fills the rest.
        // The generation dir is unioned with the legacy flat dir so committed
        // packs from before generation namespacing keep serving.
        let reuse = match (
            self.options.vectors_dir.as_ref(),
            self.options.vectors_model_id.as_deref(),
        ) {
            (Some(dir), Some(model)) => crate::embed::sidecar::load_reuse_map_with_legacy(
                dir,
                self.options.vectors_legacy_dir.as_deref(),
                model,
                768,
                crate::embed::sidecar::CODE_TEXT_RECIPE,
            )?,
            _ => HashMap::new(),
        };
        self.build_name_rows(&ktx, &reuse)?;
        self.build_symbol_doc_rows(&ktx)?;
        self.build_file_doc_rows(&ktx)?;
        ktx.commit().map_err(be)?;
        self.build_name_words()?;
        self.build_entity_stats()?;
        Ok(())
    }

    /// Build-time entity counts into `stats` (build-time-stats): symbols /
    /// files / defs per language split by subset (internal/test/external), and
    /// packages per manager (internal/external). One pass of `GROUP BY`
    /// aggregations — all counting happens here, never on the read path.
    fn build_entity_stats(&self) -> Result<(), DbError> {
        // external takes precedence over test; matches the symbol/file flags.
        const SUBSET: &str =
            "CASE WHEN external=1 THEN 'external' WHEN test=1 THEN 'test' ELSE 'internal' END";
        let mut rows: Vec<StatRow> = Vec::new();
        self.collect_stats(
            &mut rows,
            "language",
            "symbols",
            &format!("SELECT language, {SUBSET}, count(*) FROM symbols GROUP BY 1, 2"),
        )?;
        self.collect_stats(
            &mut rows,
            "language",
            "files",
            &format!("SELECT language, {SUBSET}, count(*) FROM files GROUP BY 1, 2"),
        )?;
        // defs inherit their symbol's language + subset.
        self.collect_stats(
            &mut rows,
            "language",
            "defs",
            "SELECT s.language, \
             CASE WHEN s.external=1 THEN 'external' WHEN s.test=1 THEN 'test' ELSE 'internal' END, \
             count(*) FROM defs d JOIN symbols s ON s.id = d.sym_id GROUP BY 1, 2",
        )?;
        // packages carry `external` but no `test`.
        self.collect_stats(
            &mut rows,
            "manager",
            "packages",
            "SELECT manager, CASE WHEN external=1 THEN 'external' ELSE 'internal' END, count(*) \
             FROM packages GROUP BY 1, 2",
        )?;
        self.write_stats(&rows)
    }

    /// Run a `(key, subset, count)` aggregation and append the rows under
    /// `scope`/`metric` to `out`.
    fn collect_stats(
        &self,
        out: &mut Vec<StatRow>,
        scope: &str,
        metric: &str,
        sql: &str,
    ) -> Result<(), DbError> {
        let mut q = self.graph.prepare(sql).map_err(be)?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .map_err(be)?;
        for row in rows {
            let (key, subset, value) = row.map_err(be)?;
            out.push(StatRow {
                scope: scope.to_owned(),
                key,
                subset,
                metric: metric.to_owned(),
                value,
            });
        }
        Ok(())
    }

    /// Populate `code.db`'s `name_words` FTS5 index: one row per symbol,
    /// `words` = the symbol name split into lowercase words, rowid-aligned to
    /// `symbols.rowid` so the reader recovers the full row by join (design D6).
    fn build_name_words(&self) -> Result<(), DbError> {
        use super::super::super::codes::split_identifier;

        let names: Vec<(i64, String)> = {
            let mut q = self
                .graph
                .prepare("SELECT rowid, name FROM symbols")
                .map_err(be)?;
            let rows = q
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .map_err(be)?;
            rows.collect::<Result<_, _>>().map_err(be)?
        };
        let gtx = self.graph.unchecked_transaction().map_err(be)?;
        {
            let mut ins = gtx
                .prepare_cached("INSERT INTO name_words(rowid, words) VALUES(?, ?)")
                .map_err(be)?;
            for (rowid, name) in names {
                ins.execute(params![rowid, split_identifier(&name)])
                    .map_err(be)?;
            }
        }
        gtx.commit().map_err(be)?;
        Ok(())
    }

    /// One `knowledge` name row per symbol. `name_text` (the trigram FTS
    /// field) is the split signature only — the lexical sig arm. The vector,
    /// though, embeds the **doc prose only** (the `doc` recipe), so its
    /// fingerprint tracks the doc text alone: a signature-only edit no longer
    /// invalidates the vector, and an **undocumented symbol gets no vector** at
    /// all (it is found lexically). A committed sidecar vector for a documented
    /// symbol's doc fingerprint is reconciled into `vec0`.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "xxh3 u64 fingerprint stored as its i64 bit pattern; round-trips losslessly"
    )]
    fn build_name_rows(
        &self,
        ktx: &Transaction,
        reuse: &HashMap<u64, QuantVector>,
    ) -> Result<(), DbError> {
        use super::super::super::codes::{split_identifier, text_fingerprint};

        let mut ins = ktx
            .prepare_cached(
                "INSERT INTO knowledge(embed_key,id,row_kind,language,pub_id,path,name,kind,\
                 name_text,doc_text,fingerprint) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            )
            .map_err(be)?;
        let mut name_fts = ktx
            .prepare_cached("INSERT INTO name_fts(rowid,name_text) VALUES(?,?)")
            .map_err(be)?;
        let mut vec_ins = ktx
            .prepare_cached("INSERT INTO vec_knowledge(rowid, embedding) VALUES(?,?)")
            .map_err(be)?;
        let mut q = self
            .graph
            .prepare(
                // The doc subquery skips empty docs (`doc <> ''`) so it picks the
                // SAME first-non-empty doc that `db::jobs::scan_rows` embeds (the
                // min-rowid knowledge doc row). Otherwise a multi-doc symbol whose
                // first `symbol_docs` row is empty would fingerprint `fp("")` while
                // its real doc gets embedded — colliding with every undocumented
                // symbol's `fp("")` on reuse.
                "SELECT s.id, s.pub_id, s.language, s.kind, s.name, \
                 COALESCE((SELECT sig FROM symbol_docs WHERE sym_id = s.id LIMIT 1), ''), \
                 COALESCE((SELECT doc FROM symbol_docs WHERE sym_id = s.id AND doc <> '' LIMIT 1), '') \
                 FROM symbols s",
            )
            .map_err(be)?;
        let names = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                ))
            })
            .map_err(be)?;
        for row in names {
            let (id, pub_id, lang, kind, name, sig, doc) = row.map_err(be)?;
            let raw = if sig.is_empty() {
                format!("{kind} {name}")
            } else {
                sig
            };
            let text = split_identifier(&raw);
            // Doc-only recipe: the vector embeds the doc prose only, so the
            // fingerprint tracks `doc` alone. `embed_pending` reconstructs the
            // same text from the `doc_text` column.
            let fp_u64 = text_fingerprint(&doc);
            let fp = fp_u64 as i64;
            let embed_key = format!("name:{lang}:{pub_id}");
            ins.execute(params![
                embed_key,
                id,
                "name",
                lang,
                pub_id,
                None::<String>,
                name,
                kind,
                text,
                None::<String>,
                fp
            ])
            .map_err(be)?;
            let rowid = ktx.last_insert_rowid();
            name_fts.execute(params![rowid, text]).map_err(be)?;
            // Only documented symbols embed; reconcile a committed doc vector.
            if !doc.is_empty() {
                if let Some(qv) = reuse.get(&fp_u64) {
                    let bytes: Vec<u8> = qv
                        .dequantize()
                        .iter()
                        .flat_map(|f| f.to_le_bytes())
                        .collect();
                    vec_ins.execute(params![rowid, bytes]).map_err(be)?;
                }
            }
        }
        Ok(())
    }

    /// One `knowledge` doc row per non-empty `symbol_docs` doc — the porter
    /// FTS prose index for symbols (no `vec0` row; only name rows embed).
    #[expect(
        clippy::cast_possible_wrap,
        reason = "xxh3 u64 fingerprint stored as its i64 bit pattern; round-trips losslessly"
    )]
    fn build_symbol_doc_rows(&self, ktx: &Transaction) -> Result<(), DbError> {
        use super::super::super::codes::text_fingerprint;

        let mut ins = ktx
            .prepare_cached(
                "INSERT INTO knowledge(embed_key,id,row_kind,language,pub_id,path,name,kind,\
                 name_text,doc_text,fingerprint) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            )
            .map_err(be)?;
        let mut doc_fts = ktx
            .prepare_cached("INSERT INTO doc_fts(rowid,doc_text) VALUES(?,?)")
            .map_err(be)?;
        let mut q = self
            .graph
            .prepare(
                "SELECT s.id, s.pub_id, s.language, s.kind, s.name, d.doc \
                 FROM symbol_docs d JOIN symbols s ON s.id = d.sym_id WHERE d.doc <> ''",
            )
            .map_err(be)?;
        let docs = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })
            .map_err(be)?;
        for row in docs {
            let (id, pub_id, lang, kind, name, doc) = row.map_err(be)?;
            let fp = text_fingerprint(&doc) as i64;
            let embed_key = format!("doc:{lang}:{pub_id}");
            ins.execute(params![
                embed_key,
                id,
                "doc",
                lang,
                pub_id,
                None::<String>,
                name,
                kind,
                None::<String>,
                doc,
                fp
            ])
            .map_err(be)?;
            let rowid = ktx.last_insert_rowid();
            doc_fts.execute(params![rowid, doc]).map_err(be)?;
        }
        Ok(())
    }

    /// One `knowledge` doc row per non-empty `file_docs` entry, path-identified
    /// (empty `pub_id` — these surface as file hits, not symbol hits).
    #[expect(
        clippy::cast_possible_wrap,
        reason = "xxh3 u64 fingerprint stored as its i64 bit pattern; round-trips losslessly"
    )]
    fn build_file_doc_rows(&self, ktx: &Transaction) -> Result<(), DbError> {
        use super::super::super::codes::text_fingerprint;

        let mut ins = ktx
            .prepare_cached(
                "INSERT INTO knowledge(embed_key,id,row_kind,language,pub_id,path,name,kind,\
                 name_text,doc_text,fingerprint) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            )
            .map_err(be)?;
        let mut doc_fts = ktx
            .prepare_cached("INSERT INTO doc_fts(rowid,doc_text) VALUES(?,?)")
            .map_err(be)?;
        let mut q = self
            .graph
            .prepare(
                "SELECT f.id, f.language, f.path, fd.doc \
                 FROM file_docs fd JOIN files f ON f.id = fd.file_id WHERE fd.doc <> ''",
            )
            .map_err(be)?;
        let fdocs = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(be)?;
        for row in fdocs {
            let (id, lang, path, doc) = row.map_err(be)?;
            let fp = text_fingerprint(&doc) as i64;
            let embed_key = format!("filedoc:{lang}:{path}");
            ins.execute(params![
                embed_key,
                id,
                "doc",
                lang,
                "",
                path,
                None::<String>,
                None::<String>,
                None::<String>,
                doc,
                fp
            ])
            .map_err(be)?;
            let rowid = ktx.last_insert_rowid();
            doc_fts.execute(params![rowid, doc]).map_err(be)?;
        }
        Ok(())
    }
}
