//! `SQLite` reader (replace-lance-with-sqlite, task 1.3 / Phase 3).
//!
//! The reader holds no resident projection — only the snapshot path. Every
//! operation opens a short-lived read-only connection and serves itself from
//! `code.db` / `vector.db` indexes: point fetches by id, traversal via the
//! `edges_src`/`edges_tgt` indexes, location via `defs_file`, search via FTS5 /
//! `vec0`. This module holds the struct + `open` + the connection openers +
//! the shared row-mapping / hydration / scoring helpers; the method groups and
//! the [`crate::api::Reader`] impl are split across the sibling submodules.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::api::types::{BlendedHit, DbError, RankedSymbolRow, SymbolRow};

/// Whole-name exact-match boost over n-gram hits in the single-arm identifier
/// path (`search_symbols_by_name`). The blended fusion uses [`EXACT_BONUS`].
pub(super) const EXACT_BOOST: f64 = 10.0;

/// Reciprocal-Rank-Fusion damping constant (design D1): each arm contributes
/// `w / (RRF_K + rank)`, fusing by rank rather than raw score so the
/// unbounded-BM25 / bounded-cosine scale fight disappears.
pub(super) const RRF_K: f64 = 60.0;
/// RRF arm weights (design D1/D3, spike-tuned). The `name_lower` identifier,
/// signature-trigram, and vector arms weigh 1.0; the doc-prose arm 0.7.
pub(super) const W_NAME_LOWER: f64 = 1.0;
pub(super) const W_SIG: f64 = 1.0;
pub(super) const W_DOC: f64 = 0.7;
pub(super) const W_VEC: f64 = 1.0;
/// Additive exact-name bonus (design D2): sized to dominate the max RRF sum
/// (≈ Σ w/(K+1) ≈ 0.06) so an exact `name_lower == query` hit always ranks first.
pub(super) const EXACT_BONUS: f64 = 1.0;

/// Bind a `usize` pool/limit count to a `SQLite` `LIMIT` parameter.
#[expect(
    clippy::cast_possible_wrap,
    reason = "a small positive pool/limit count always fits an i64 LIMIT bind"
)]
pub(super) fn lim(n: usize) -> i64 {
    n as i64
}

/// Fold one arm's ordered candidate list (best-first) into the RRF score map:
/// the id at 0-based position `i` gains `w / (RRF_K + i + 1)` (design D1/D5).
#[expect(
    clippy::cast_precision_loss,
    reason = "arm ranks are tiny candidate-pool counts, well under 2^52"
)]
pub(super) fn rrf_into(scores: &mut HashMap<u32, f64>, arm: &[u32], w: f64) {
    for (i, id) in arm.iter().enumerate() {
        *scores.entry(*id).or_default() += w / (RRF_K + (i + 1) as f64);
    }
}

/// Build an injection-safe FTS5 MATCH expression for a **trigram** arm: the
/// query as one quoted literal (substring search), with embedded `"` doubled.
/// Returns `None` when the query has fewer than 3 alphanumeric chars — the
/// trigram tokenizer needs ≥3 (design D8).
pub(super) fn fts5_match_trigram(query: &str) -> Option<String> {
    if query.chars().filter(|c| c.is_alphanumeric()).count() < 3 {
        return None;
    }
    let escaped = query.replace('"', "\"\"");
    Some(format!("\"{escaped}\""))
}

/// Build an injection-safe FTS5 MATCH expression for a **word** arm (porter /
/// `unicode61`): split the query into identifier words, quote each, OR-join
/// (design D7/D8). `split_identifier` lowercases and drops non-alphanumerics,
/// so the tokens are quote-safe by construction. Returns `None` when no usable
/// token remains.
pub(super) fn fts5_match_words(query: &str) -> Option<String> {
    let expr = crate::db::codes::split_identifier(query)
        .split_whitespace()
        .map(|w| format!("\"{w}\""))
        .collect::<Vec<_>>()
        .join(" OR ");
    (!expr.is_empty()).then_some(expr)
}

/// Whether a table/virtual-table named `name` exists in `conn` — lets the
/// word-split arm skip cleanly on snapshots indexed before `name_words` existed.
pub(super) fn table_exists(conn: &Connection, name: &str) -> Result<bool, DbError> {
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = ?1",
            [name],
            |r| r.get(0),
        )
        .map_err(be)?;
    Ok(n > 0)
}

/// Read column `idx` as a `u32`. `SQLite` stores ids as signed `i64`; kenn ids
/// are non-negative and bounded to `u32`, so the cast is lossless.
#[expect(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "SQLite stores ids as i64; kenn ids are non-negative u32, so the cast is lossless"
)]
pub(super) fn col_u32(r: &rusqlite::Row, idx: usize) -> rusqlite::Result<u32> {
    Ok(r.get::<_, i64>(idx)? as u32)
}

/// Read column `idx` as a `u64`. `SQLite` stores these counts/degrees as
/// signed `i64`; they are non-negative, so the cast is lossless.
#[expect(
    clippy::cast_sign_loss,
    reason = "SQLite stores weights/degrees as i64; they are non-negative, so the cast is lossless"
)]
pub(super) fn col_u64(r: &rusqlite::Row, idx: usize) -> rusqlite::Result<u64> {
    Ok(r.get::<_, i64>(idx)? as u64)
}

/// Read column `idx` as an `f32` (a ratio stored as `SQLite` `REAL`/`f64`).
#[expect(
    clippy::cast_possible_truncation,
    reason = "ratios in [0,1] round-trip to f32 with ample precision"
)]
pub(super) fn col_f32(r: &rusqlite::Row, idx: usize) -> rusqlite::Result<f32> {
    Ok(r.get::<_, f64>(idx)? as f32)
}

/// Row-level narrowing. Visibility (external/test) plus the predicates every
/// `list` command accepted and previously dropped on the floor: package, kind
/// and language. Applied BEFORE `limit`, so a narrowed page is still a full
/// page.
///
/// `pkg_ids` is the resolved form of `narrow.packages` — `None` means no
/// package narrowing, `Some(empty)` means the requested names matched no
/// package and therefore nothing passes.
pub(super) fn passes_filter(
    s: &SymbolRow,
    narrow: &crate::api::types::RowNarrow,
    pkg_ids: Option<&std::collections::HashSet<u32>>,
) -> bool {
    if (s.external && !narrow.include_external) || (s.test && !narrow.include_tests) {
        return false;
    }
    if let Some(ids) = pkg_ids {
        if !ids.contains(&s.pkg_id) {
            return false;
        }
    }
    if let Some(kinds) = &narrow.kinds {
        if !kinds.iter().any(|k| k == &s.kind) {
            return false;
        }
    }
    if let Some(langs) = &narrow.languages {
        if !langs.iter().any(|l| l == &s.language) {
            return false;
        }
    }
    true
}

/// The blended composite score of a hit — for ranking symbol and file hits
/// in one list.
pub(super) fn hit_score(hit: &BlendedHit) -> f64 {
    match hit {
        BlendedHit::Symbol(s) => s.score,
        BlendedHit::File(f) => f.score,
    }
}

/// Hydrate a resident [`SymbolRow`] into a [`RankedSymbolRow`] with `score`.
pub(super) fn ranked(s: &SymbolRow, score: f64) -> RankedSymbolRow {
    RankedSymbolRow {
        id: s.id,
        pub_id: s.pub_id.clone(),
        language: s.language.clone(),
        pkg_id: s.pkg_id,
        kind: s.kind.clone(),
        name: s.name.clone(),
        partial: s.partial,
        nargs: s.nargs,
        targs: s.targs,
        external: s.external,
        test: s.test,
        enclosing_sym_id: s.enclosing_sym_id,
        score,
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a map_err fn pointer, which passes the error by value"
)]
pub(super) fn be(e: rusqlite::Error) -> DbError {
    DbError::Backend(format!("sqlite: {e}"))
}

/// The standard graph tables `count_table` reports on — also the whitelist
/// that keeps its `count(*)` query name-safe. Names from [`crate::db::names`].
pub(super) const COUNT_TABLES: &[&str] = &[
    crate::db::names::code::FILES,
    crate::db::names::code::PACKAGES,
    crate::db::names::code::SYMBOLS,
    crate::db::names::code::SYMBOL_DOCS,
    crate::db::names::code::FILE_DOCS,
    crate::db::names::code::DEFS,
    crate::db::names::code::EDGES,
];

/// Number of background-thread connections in a reader pool. Small and fixed:
/// enough to parallelize concurrent MCP reads without spawning a connection
/// per logical CPU for an otherwise-idle server (mcp-offload-blocking-storage).
const READER_POOL_CONNS: usize = 4;

/// Busy timeout per pooled connection — the background embed pass
/// (`db::jobs::embed_pending`) fills `vec0` in `vector.db` in place, so a read
/// waits out its brief commit lock rather than failing with `SQLITE_BUSY`.
const READER_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// A reader over one published snapshot — an async-sqlite connection [`Pool`].
/// Each pooled connection opens `code.db` read-only as `main` with `vector.db`
/// attached read-only (`AS vec`), so a single `Pool::conn` closure can query
/// the graph, FTS5, and `vec0` and even join across them: the two files share
/// no table names, so bare names resolve across the attach. Queries run on the
/// pool's background threads (off the runtime workers), and the N connections
/// serve N concurrent reads in parallel.
///
/// [`Pool`]: async_sqlite::Pool
#[derive(Clone)]
pub(crate) struct SqliteReader {
    pub(super) pool: async_sqlite::Pool,
}

/// A borrowed view over one pooled `&Connection`, live for the duration of a
/// `Pool::conn` closure. The synchronous query methods (fetch / scan / search /
/// traversal) run against it. [`Self::graph`] and [`Self::knowledge`] both
/// return the same connection (`vector.db` is attached), so the call sites that
/// historically used two logical connections keep their shape.
pub(crate) struct SqliteConnRef<'a> {
    /// The pooled connection — `code.db` (main) with `vector.db` attached, so
    /// the graph, FTS5, and `vec0` tables all resolve through it by bare name.
    pub(super) conn: &'a Connection,
}

/// The `SELECT` column list shared by [`symbol_from_row`] — used by the
/// by-id batch hydration, the by-`(lang,pub_id)` point fetch, and the
/// whole-table scan.
pub(super) const SYMBOL_COLS: &str = "id,pub_id,language,pkg_id,kind,name,partial,nargs,targs,\
     external,test,enclosing_sym_id";

/// Hydrate a [`SymbolRow`] from a row of [`SYMBOL_COLS`].
pub(super) fn symbol_from_row(r: &rusqlite::Row) -> rusqlite::Result<SymbolRow> {
    Ok(SymbolRow {
        id: col_u32(r, 0)?,
        pub_id: r.get(1)?,
        language: r.get(2)?,
        pkg_id: col_u32(r, 3)?,
        kind: r.get(4)?,
        name: r.get(5)?,
        partial: r.get::<_, i64>(6)? != 0,
        nargs: r.get(7)?,
        targs: r.get(8)?,
        external: r.get::<_, i64>(9)? != 0,
        test: r.get::<_, i64>(10)? != 0,
        enclosing_sym_id: col_u32(r, 11)?,
    })
}

/// Hydrate a [`FileRow`] from `id, path, language, test, external`.
pub(crate) fn file_from_row(r: &rusqlite::Row) -> rusqlite::Result<crate::api::types::FileRow> {
    Ok(crate::api::types::FileRow {
        id: col_u32(r, 0)?,
        path: r.get(1)?,
        language: r.get(2)?,
        test: r.get::<_, i64>(3)? != 0,
        external: r.get::<_, i64>(4)? != 0,
    })
}

/// Hydrate a [`PackageRow`] from `id, name, version, manager, external`.
pub(super) fn package_from_row(
    r: &rusqlite::Row,
) -> rusqlite::Result<crate::api::types::PackageRow> {
    Ok(crate::api::types::PackageRow {
        id: col_u32(r, 0)?,
        name: r.get(1)?,
        version: r.get(2)?,
        manager: r.get(3)?,
        external: r.get::<_, i64>(4)? != 0,
    })
}

/// Max ids bound per `WHERE id IN (...)` query — under `SQLite`'s default
/// `SQLITE_MAX_VARIABLE_NUMBER` (999 on older builds), so large id sets are
/// hydrated in a few chunked queries rather than one over-limit one.
const ID_CHUNK: usize = 900;

/// Batch-hydrate symbol rows by id via chunked `WHERE id IN (...)` queries.
/// Returns an `id → row` map; ids absent from `symbols` are simply missing. An
/// empty `ids` returns an empty map without touching the database.
pub(super) fn fetch_symbols_by_ids(
    conn: &Connection,
    ids: &[u32],
) -> Result<HashMap<u32, SymbolRow>, DbError> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(ID_CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let mut stmt = conn
            .prepare_cached(&format!(
                "SELECT {SYMBOL_COLS} FROM symbols WHERE id IN ({placeholders})"
            ))
            .map_err(be)?;
        let params: Vec<i64> = chunk.iter().map(|&id| i64::from(id)).collect();
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params), symbol_from_row)
            .map_err(be)?;
        for row in rows {
            let s = row.map_err(be)?;
            out.insert(s.id, s);
        }
    }
    Ok(out)
}

impl SqliteReader {
    /// Open a read-only connection pool over the snapshot. Each connection
    /// opens `code.db` (main) and attaches `vector.db` read-only as `vec`, with
    /// `vec0` registered process-globally and a busy timeout. A missing/corrupt
    /// `code.db` fails the pool open; a missing `vector.db` fails the attach —
    /// both surface here, not on the first query.
    pub(crate) async fn open(snapshot: &Path) -> Result<Self, DbError> {
        super::super::ensure_vec_extension();
        let pool = async_sqlite::PoolBuilder::new()
            .path(snapshot.join(crate::db::names::CODE_DB))
            .flags(OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)
            .num_conns(READER_POOL_CONNS)
            .open()
            .await?;
        let attach = format!(
            "ATTACH DATABASE 'file:{}?mode=ro' AS vec",
            snapshot.join(crate::db::names::VECTOR_DB).display()
        );
        // Configure every pooled connection once: busy timeout + the vector.db
        // attach. Re-established per pool build (i.e. per snapshot bind).
        for r in pool
            .conn_for_each(move |c| {
                c.busy_timeout(READER_BUSY_TIMEOUT)?;
                c.execute_batch(&attach)
            })
            .await
        {
            r?;
        }
        Ok(Self { pool })
    }
}

#[cfg(test)]
mod narrow_tests {
    use super::passes_filter;
    use crate::api::types::{RowNarrow, SymbolRow};

    fn row(pkg: u32, kind: &str, lang: &str) -> SymbolRow {
        SymbolRow {
            id: 1,
            pub_id: "rs:x".into(),
            language: lang.into(),
            pkg_id: pkg,
            kind: kind.into(),
            name: "x".into(),
            partial: false,
            nargs: 0,
            targs: 0,
            external: false,
            test: false,
            enclosing_sym_id: 0,
        }
    }

    /// `package`, `kind` and `language` were accepted by every `list` command
    /// and dropped on the floor, so a narrowed query returned the unnarrowed
    /// list — indistinguishable from a genuine answer. Mutation-checked:
    /// deleting any one arm lets a non-matching row through.
    #[test]
    fn every_predicate_narrows() {
        let r = row(7, "method", "rust");
        let base = RowNarrow::visibility(false, false);
        assert!(passes_filter(&r, &base, None), "no narrowing → passes");

        let mut n = base.clone();
        n.kinds = Some(vec!["method".into()]);
        assert!(passes_filter(&r, &n, None));
        n.kinds = Some(vec!["class".into()]);
        assert!(!passes_filter(&r, &n, None), "kind narrows");

        let mut n = base.clone();
        n.languages = Some(vec!["csharp".into()]);
        assert!(!passes_filter(&r, &n, None), "language narrows");

        let hit: std::collections::HashSet<u32> = [7].into_iter().collect();
        let miss: std::collections::HashSet<u32> = [9].into_iter().collect();
        assert!(passes_filter(&r, &base, Some(&hit)));
        assert!(!passes_filter(&r, &base, Some(&miss)), "package narrows");
    }

    /// A package name that matches nothing must match NOTHING — an empty id set
    /// is a real answer, not "no filter". Getting this backwards would make a
    /// typo silently return the whole list.
    #[test]
    fn an_unmatched_package_name_excludes_everything() {
        let empty: std::collections::HashSet<u32> = std::collections::HashSet::new();
        assert!(!passes_filter(
            &row(7, "method", "rust"),
            &RowNarrow::visibility(false, false),
            Some(&empty)
        ));
    }

    /// Visibility still wins regardless of the other predicates.
    #[test]
    fn visibility_is_independent() {
        let mut r = row(7, "method", "rust");
        r.test = true;
        let mut n = RowNarrow::visibility(false, false);
        n.kinds = Some(vec!["method".into()]);
        assert!(!passes_filter(&r, &n, None), "a test row is still excluded");
    }
}
