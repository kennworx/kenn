//! FTS5 / `vec0` search inherent methods on [`SqliteConn`].

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension};

use super::projection::{
    be, col_u32, fetch_symbols_by_ids, fts5_match_trigram, fts5_match_words, hit_score, lim,
    passes_filter, ranked, rrf_into, symbol_from_row, table_exists, SqliteConnRef, EXACT_BONUS,
    EXACT_BOOST, SYMBOL_COLS, W_DOC, W_NAME_LOWER, W_SIG, W_VEC,
};
use crate::api::types::{
    BlendedFileRow, BlendedHit, BlendedSymbolRow, DbError, FoundSymbolRow, MatchKind,
    RankedSymbolRow, RowNarrow, SymbolRow,
};
use kenn_model::ShortId;

impl SqliteConnRef<'_> {
    /// Identifier search: FTS5 trigram candidates over `name_fts`, re-ranked
    /// with the exact-match boost + `(score DESC, name-len ASC, id ASC)`
    /// tiebreak (task 4.1). Returns up to `limit` ranked symbols.
    pub(crate) fn search_symbols_by_name(
        &self,
        query: &str,
        limit: u32,
        include_external: bool,
        include_tests: bool,
    ) -> Result<Vec<RankedSymbolRow>, DbError> {
        let Some(m) = fts5_match_trigram(query) else {
            return Ok(Vec::new()); // trigram tokenizer needs ≥3 alphanumeric chars
        };
        let pool = (limit as usize * 4).max(50);
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT k.id, bm25(name_fts) FROM name_fts \
                 JOIN knowledge k ON k.rowid = name_fts.rowid \
                 WHERE name_fts MATCH ?1 ORDER BY bm25(name_fts) LIMIT ?2",
            )
            .map_err(be)?;
        let cands: Vec<(u32, f64)> = stmt
            .query_map(rusqlite::params![m, lim(pool)], |r| {
                Ok((col_u32(r, 0)?, r.get::<_, f64>(1)?))
            })
            .map_err(be)?
            .collect::<Result<_, _>>()
            .map_err(be)?;
        drop(stmt);
        let ids: Vec<u32> = cands.iter().map(|(id, _)| *id).collect();
        let symbols = fetch_symbols_by_ids(self.conn, &ids)?;

        let ql = query.to_ascii_lowercase();
        let mut hits: Vec<RankedSymbolRow> = Vec::new();
        for (id, bm25) in cands {
            let Some(s) = symbols.get(&id) else {
                continue;
            };
            if !passes_filter(
                s,
                &RowNarrow::visibility(include_external, include_tests),
                None,
            ) {
                continue;
            }
            // bm25() is lower-is-better (≤0); negate so higher is better.
            let mut score = -bm25;
            if s.name.to_ascii_lowercase() == ql {
                score += EXACT_BOOST;
            }
            hits.push(ranked(s, score));
        }
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.name.len().cmp(&b.name.len()))
                .then_with(|| a.id.cmp(&b.id))
        });
        hits.truncate(limit as usize);
        Ok(hits)
    }

    /// Tiered identifier lookup (design D6): Tier 1 whole-name exact matches
    /// (indexed `name_lower`), Tier 2 the separator-agnostic word-split
    /// `name_words` arm, Tier 3 n-gram/substring FTS — deduped in that priority
    /// order, each hit tagged with its [`MatchKind`], truncated to `limit`.
    pub(crate) fn find_symbol_tiered(
        &self,
        name: &str,
        limit: u32,
        include_external: bool,
        include_tests: bool,
    ) -> Result<Vec<FoundSymbolRow>, DbError> {
        let mut seen = HashSet::new();
        let mut out: Vec<FoundSymbolRow> = Vec::new();
        // Tier 1: whole-name exact matches, case-insensitive. Query the indexed
        // `name_lower` column (`symbols_name_lower`) rather than `name … COLLATE
        // NOCASE` (which can't use an index); matching the writer's lowercasing.
        let exact: Vec<SymbolRow> = {
            let mut stmt = self
                .conn
                .prepare_cached(&format!(
                    "SELECT {SYMBOL_COLS} FROM symbols WHERE name_lower = ?1"
                ))
                .map_err(be)?;
            let rows = stmt
                .query_map(rusqlite::params![name.to_lowercase()], symbol_from_row)
                .map_err(be)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(be)?;
            rows
        };
        for s in exact {
            if passes_filter(
                &s,
                &RowNarrow::visibility(include_external, include_tests),
                None,
            ) && seen.insert(s.id)
            {
                out.push(FoundSymbolRow {
                    symbol: s,
                    match_kind: MatchKind::Exact,
                });
            }
        }
        // Tier 2: separator-agnostic word-split match (`name_words`, unicode61)
        // — finds `cancel_order`/`CancelOrder` by the words `cancel order`.
        for s in self.search_symbols_by_words(name, limit, include_external, include_tests)? {
            if seen.insert(s.id) {
                out.push(FoundSymbolRow {
                    symbol: s,
                    match_kind: MatchKind::Fuzzy,
                });
            }
        }
        // Tier 3: n-gram / substring matches from FTS5.
        for r in self.search_symbols_by_name(name, limit, include_external, include_tests)? {
            if seen.insert(r.id) {
                out.push(FoundSymbolRow {
                    symbol: SymbolRow::from(r),
                    match_kind: MatchKind::Contains,
                });
            }
        }
        out.truncate(limit as usize);
        Ok(out)
    }

    /// Blended search: Reciprocal Rank Fusion (K=60) over four arms —
    /// `name_lower` identifier (exact→prefix→contains), `name_fts` signature
    /// trigram, `doc_fts` prose (symbol docs only — file docs excluded), and
    /// optional `vec0` cosine — plus an additive exact-name bonus so an exact
    /// identifier always ranks first (design D1/D2/D3). Returns up to
    /// `target_total` symbols ranked `(score DESC, name-len ASC, id ASC)`.
    /// `query_vec = None` ⇒ the vector arm is empty (lexical-only).
    pub(crate) fn search_symbols_blended(
        &self,
        query: &str,
        query_vec: Option<&[f32]>,
        target_total: u32,
        include_external: bool,
        include_tests: bool,
    ) -> Result<Vec<BlendedSymbolRow>, DbError> {
        let pool = (target_total as usize * 8).max(64);

        // Lexical + vector arms over vector.db → ordered candidate lists.
        let sig = name_fts_candidates(self.conn, query, pool)?;
        let doc = doc_fts_candidates(self.conn, query, pool)?;
        let vec_arm = match query_vec {
            Some(v) => vector_candidates(self.conn, v, pool)?,
            None => Vec::new(),
        };

        // Identifier arm + hydration over code.db (same pooled connection).
        let nl = name_lower_candidates(self.conn, query, pool, include_external, include_tests)?;
        let ids: Vec<u32> = nl
            .iter()
            .chain(&sig)
            .chain(&doc)
            .chain(&vec_arm)
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let symbols = fetch_symbols_by_ids(self.conn, &ids)?;

        let mut scores: HashMap<u32, f64> = HashMap::new();
        rrf_into(&mut scores, &nl, W_NAME_LOWER);
        rrf_into(&mut scores, &sig, W_SIG);
        rrf_into(&mut scores, &doc, W_DOC);
        rrf_into(&mut scores, &vec_arm, W_VEC);

        let ql = query.to_ascii_lowercase();
        let mut hits: Vec<BlendedSymbolRow> = scores
            .into_iter()
            .filter_map(|(id, mut score)| {
                let s = symbols.get(&id)?;
                if !passes_filter(
                    s,
                    &RowNarrow::visibility(include_external, include_tests),
                    None,
                ) {
                    return None;
                }
                if s.name.to_ascii_lowercase() == ql {
                    score += EXACT_BONUS;
                }
                Some(BlendedSymbolRow {
                    symbol: s.clone(),
                    score,
                })
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.symbol.name.len().cmp(&b.symbol.name.len()))
                .then_with(|| a.symbol.id.cmp(&b.symbol.id))
        });
        hits.truncate(target_total as usize);
        Ok(hits)
    }

    /// Separator-agnostic word-split identifier candidates over the `name_words`
    /// FTS5 index (`unicode61`, design D6): the query is split into identifier
    /// words, OR-combined, and BM25-ranked, so `cancel_order` / `CancelOrder` /
    /// `cancel-order` all match `cancel order`. Lives in the identifier path
    /// (`find_symbol_tiered`), NOT the blended fusion (it regresses conceptual
    /// ranking). Empty on snapshots indexed before `name_words` existed.
    pub(crate) fn search_symbols_by_words(
        &self,
        query: &str,
        limit: u32,
        include_external: bool,
        include_tests: bool,
    ) -> Result<Vec<SymbolRow>, DbError> {
        let Some(m) = fts5_match_words(query) else {
            return Ok(Vec::new());
        };
        if !table_exists(self.conn, "name_words")? {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT {SYMBOL_COLS} FROM name_words nw \
                 JOIN symbols s ON s.rowid = nw.rowid \
                 WHERE name_words MATCH ?1 ORDER BY bm25(name_words) LIMIT ?2"
            ))
            .map_err(be)?;
        let rows = stmt
            .query_map(rusqlite::params![m, i64::from(limit)], symbol_from_row)
            .map_err(be)?;
        let mut out = Vec::new();
        for row in rows {
            let s = row.map_err(be)?;
            if passes_filter(
                &s,
                &RowNarrow::visibility(include_external, include_tests),
                None,
            ) {
                out.push(s);
            }
        }
        Ok(out)
    }

    /// File-inclusive blended search: symbol hits interleaved with
    /// file-level-doc hits, ranked together by score. File-doc rows are the
    /// `doc_fts` rows with an empty `pub_id` (their `id`/`path` columns are
    /// the file's, not a symbol's).
    pub(crate) fn search_blended_hits(
        &self,
        query: &str,
        query_vec: Option<&[f32]>,
        target_total: u32,
        include_external: bool,
        include_tests: bool,
    ) -> Result<Vec<BlendedHit>, DbError> {
        let symbols = self.search_symbols_blended(
            query,
            query_vec,
            target_total,
            include_external,
            include_tests,
        )?;
        let files = self.search_file_docs(query, target_total)?;
        let mut hits: Vec<BlendedHit> = symbols
            .into_iter()
            .map(BlendedHit::Symbol)
            .chain(files.into_iter().map(BlendedHit::File))
            .collect();
        hits.sort_by(|a, b| hit_score(b).total_cmp(&hit_score(a)));
        hits.truncate(target_total as usize);
        Ok(hits)
    }

    /// File-level-doc hits for `query` — the `doc_fts` rows with an empty
    /// `pub_id`, scored by BM25 (lower-is-better, negated to match the
    /// symbol arm).
    #[expect(
        clippy::cast_possible_wrap,
        reason = "the candidate-pool bound is a small positive count bound to a SQLite LIMIT parameter"
    )]
    fn search_file_docs(
        &self,
        query: &str,
        target_total: u32,
    ) -> Result<Vec<BlendedFileRow>, DbError> {
        let q_escaped = query.replace('"', "");
        if q_escaped.trim().is_empty() {
            return Ok(Vec::new());
        }
        let pool = (target_total as usize * 8).max(64);
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT k.id, k.path, bm25(doc_fts) FROM doc_fts \
                 JOIN knowledge k ON k.rowid = doc_fts.rowid \
                 WHERE doc_fts MATCH ?1 AND k.pub_id = '' LIMIT ?2",
            )
            .map_err(be)?;
        let rows = stmt
            .query_map(
                rusqlite::params![format!("\"{q_escaped}\""), pool as i64],
                |r| {
                    Ok(BlendedFileRow {
                        id: col_u32(r, 0)?,
                        path: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        score: -r.get::<_, f64>(2)?,
                    })
                },
            )
            .map_err(be)?;
        rows.collect::<Result<_, _>>().map_err(be)
    }

    /// Item-to-item similar symbols (the `find_similar` MCP tool): the
    /// source symbol's own name-row vector drives a `vec0` KNN over the rest
    /// of the corpus. Returns `None` when the source has **no committed
    /// embedding** (the embed pass has not covered it — distinct from a
    /// vector that exists but has no near neighbours, which is `Some(vec![])`),
    /// so the caller can tell "vectors aren't built" from "nothing is similar."
    #[expect(
        clippy::cast_possible_wrap,
        reason = "bounded small positive count bound to a SQLite KNN parameter"
    )]
    pub(crate) fn find_similar_symbols(
        &self,
        source: ShortId,
        limit: u32,
        include_external: bool,
        include_tests: bool,
    ) -> Result<Option<Vec<RankedSymbolRow>>, DbError> {
        // The source's stored vector — its `name` row's `vec0` embedding.
        let source_vec: Option<Vec<u8>> = self
            .conn
            .prepare_cached(
                "SELECT vk.embedding FROM vec_knowledge vk \
                 JOIN knowledge k ON k.rowid = vk.rowid \
                 WHERE k.id = ?1 AND k.row_kind = 'name' LIMIT 1",
            )
            .map_err(be)?
            .query_row(rusqlite::params![i64::from(source)], |r| r.get(0))
            .optional()
            .map_err(be)?;
        let Some(source_vec) = source_vec else {
            // No committed vector for the source — signal it distinctly so the
            // caller does not read it as "no similar symbols."
            return Ok(None);
        };

        // KNN over the corpus. Over-fetch (`k`) so dropping the source's own
        // rows and the filters still leaves `limit` hits; `id` dedups the
        // name/doc rows of one symbol, keeping its best (nearest) distance.
        let k = (limit as usize * 4).max(16) as i64;
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT k.id, vk.distance FROM vec_knowledge vk \
                 JOIN knowledge k ON k.rowid = vk.rowid \
                 WHERE vk.embedding MATCH ?1 AND vk.k = ?2 ORDER BY distance",
            )
            .map_err(be)?;
        let rows = stmt
            .query_map(rusqlite::params![source_vec, k], |r| {
                Ok((col_u32(r, 0)?, r.get::<_, f64>(1)?))
            })
            .map_err(be)?;
        let mut best: HashMap<u32, f64> = HashMap::new();
        for row in rows {
            let (id, distance) = row.map_err(be)?;
            if id == source {
                continue;
            }
            best.entry(id)
                .and_modify(|d| {
                    if distance < *d {
                        *d = distance;
                    }
                })
                .or_insert(distance);
        }
        drop(stmt);
        let ids: Vec<u32> = best.keys().copied().collect();
        let symbols = fetch_symbols_by_ids(self.conn, &ids)?;

        let mut hits: Vec<RankedSymbolRow> = best
            .into_iter()
            .filter_map(|(id, distance)| {
                let s = symbols.get(&id)?;
                passes_filter(
                    s,
                    &RowNarrow::visibility(include_external, include_tests),
                    None,
                )
                .then(|| ranked(s, 1.0 - distance))
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.name.len().cmp(&b.name.len()))
                .then_with(|| a.id.cmp(&b.id))
        });
        hits.truncate(limit as usize);
        Ok(Some(hits))
    }
}

/// Signature-trigram arm: `name_fts` candidate ids, BM25-ranked best-first.
fn name_fts_candidates(conn: &Connection, query: &str, pool: usize) -> Result<Vec<u32>, DbError> {
    let Some(m) = fts5_match_trigram(query) else {
        return Ok(Vec::new());
    };
    let mut stmt = conn
        .prepare_cached(
            "SELECT k.id FROM name_fts JOIN knowledge k ON k.rowid = name_fts.rowid \
             WHERE name_fts MATCH ?1 ORDER BY bm25(name_fts) LIMIT ?2",
        )
        .map_err(be)?;
    let rows = stmt
        .query_map(rusqlite::params![m, lim(pool)], |r| col_u32(r, 0))
        .map_err(be)?;
    rows.collect::<Result<_, _>>().map_err(be)
}

/// Doc-prose arm: `doc_fts` candidate ids (symbol docs only — file-level docs
/// have an empty `pub_id` and are excluded), BM25-ranked best-first.
fn doc_fts_candidates(conn: &Connection, query: &str, pool: usize) -> Result<Vec<u32>, DbError> {
    let Some(m) = fts5_match_words(query) else {
        return Ok(Vec::new());
    };
    let mut stmt = conn
        .prepare_cached(
            "SELECT k.id, k.pub_id FROM doc_fts JOIN knowledge k ON k.rowid = doc_fts.rowid \
             WHERE doc_fts MATCH ?1 ORDER BY bm25(doc_fts) LIMIT ?2",
        )
        .map_err(be)?;
    let rows = stmt
        .query_map(rusqlite::params![m, lim(pool)], |r| {
            Ok((col_u32(r, 0)?, r.get::<_, String>(1)?))
        })
        .map_err(be)?;
    let mut out = Vec::new();
    for row in rows {
        let (id, pub_id) = row.map_err(be)?;
        if !pub_id.is_empty() {
            out.push(id);
        }
    }
    Ok(out)
}

/// Vector arm: `vec0` cosine KNN candidate ids, nearest-first, deduped to the
/// first (nearest) row per symbol. Empty until the embed job fills `vec0`.
fn vector_candidates(conn: &Connection, qv: &[f32], pool: usize) -> Result<Vec<u32>, DbError> {
    let bytes: Vec<u8> = qv.iter().flat_map(|f| f.to_le_bytes()).collect();
    let mut stmt = conn
        .prepare_cached(
            "SELECT kr.id FROM vec_knowledge JOIN knowledge kr ON kr.rowid = vec_knowledge.rowid \
             WHERE vec_knowledge.embedding MATCH ?1 AND vec_knowledge.k = ?2 ORDER BY distance",
        )
        .map_err(be)?;
    let rows = stmt
        .query_map(rusqlite::params![bytes, lim(pool)], |r| col_u32(r, 0))
        .map_err(be)?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for row in rows {
        let id = row.map_err(be)?;
        if seen.insert(id) {
            out.push(id);
        }
    }
    Ok(out)
}

/// `name_lower` identifier arm (design D3): exact → prefix → contains over the
/// indexed `symbols.name_lower`, deduped in that priority order and ordered by
/// name length within each tier. External/test rows are filtered in SQL unless
/// the corresponding `include_*` flag is set, matching the other arms' post-
/// hydration filter.
fn name_lower_candidates(
    conn: &Connection,
    query: &str,
    pool: usize,
    include_external: bool,
    include_tests: bool,
) -> Result<Vec<u32>, DbError> {
    let ql = query.to_ascii_lowercase();
    if ql.trim().is_empty() {
        return Ok(Vec::new());
    }
    // Escape LIKE metacharacters so a query with `%`/`_`/`\` stays literal.
    let esc = ql
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let mut filt = String::new();
    if !include_external {
        filt.push_str(" AND external = 0");
    }
    if !include_tests {
        filt.push_str(" AND test = 0");
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (pred, pat) in [
        ("name_lower = ?1", ql.clone()),
        ("name_lower LIKE ?1 ESCAPE '\\'", format!("{esc}%")),
        ("name_lower LIKE ?1 ESCAPE '\\'", format!("%{esc}%")),
    ] {
        let sql =
            format!("SELECT id FROM symbols WHERE {pred}{filt} ORDER BY length(name), id LIMIT ?2");
        let mut stmt = conn.prepare_cached(&sql).map_err(be)?;
        let rows = stmt
            .query_map(rusqlite::params![pat, lim(pool)], |r| col_u32(r, 0))
            .map_err(be)?;
        for row in rows {
            let id = row.map_err(be)?;
            if seen.insert(id) {
                out.push(id);
            }
        }
    }
    Ok(out)
}
