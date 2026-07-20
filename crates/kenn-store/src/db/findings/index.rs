//! Persistent findings search index — a derived `SQLite` FTS5 table over
//! committed finding text, living in the run's derived store
//! (`<derived_root>/findings.db`), NOT committed to git.
//!
//! The index is **built at store open** (from the committed `.md` records)
//! and **maintained on writes** (`push_finding` / `merge_findings`). Reads
//! ([`search_lexical`], [`live_records`]) open a fresh read-only connection
//! and query it — they MUST NOT create tables or build an index on the read
//! path (that was the old `search_findings` anti-pattern: a transient
//! in-memory FTS rebuilt from every record on every call).
//!
//! Lifecycle (`superseded` / `tombstoned`) is stamped as columns on the
//! `findings` data table and applied as a `JOIN` filter, so a supersede or
//! tombstone is a cheap `UPDATE`, never an FTS rewrite.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::api::types::{DbError, Finding};

use super::lifecycle::{carries_lifecycle_tag, lifecycle_sets};

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a map_err fn pointer, which passes the error by value"
)]
fn be(e: rusqlite::Error) -> DbError {
    DbError::Backend(format!("findings index sqlite: {e}"))
}

const SCHEMA: &str = "\
CREATE TABLE findings(\
  id TEXT PRIMARY KEY, text TEXT NOT NULL, \
  superseded INTEGER NOT NULL DEFAULT 0, tombstoned INTEGER NOT NULL DEFAULT 0);\
CREATE VIRTUAL TABLE f USING fts5(id UNINDEXED, text);";

/// Split a free-text query into quoted FTS terms (so user input can't
/// inject FTS operators), OR-joined. Mirrors the prior `search_findings`
/// tokenizer. Returns `None` when the query has no usable term.
fn match_expr(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

fn open_rw(db_path: &Path) -> Result<Connection, DbError> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(DbError::Io)?;
    }
    Connection::open(db_path).map_err(be)
}

/// `superseded`/`tombstoned` flags for a finding, given the precomputed
/// lifecycle sets. A finding that itself carries a `tombstone:` tag is
/// excluded from results just like a tombstoned target.
fn flags(
    f: &Finding,
    superseded: &std::collections::HashSet<String>,
    tombstoned: &std::collections::HashSet<String>,
) -> (i64, i64) {
    let sup = i64::from(superseded.contains(&f.id));
    let tomb = i64::from(tombstoned.contains(&f.id) || carries_lifecycle_tag(f, "tombstone:"));
    (sup, tomb)
}

/// Rebuild the index from `findings` (committed records, plus any pending).
/// Drops and repopulates — called at store open, never on a read.
pub(super) fn rebuild(db_path: &Path, findings: &[Finding]) -> Result<(), DbError> {
    let conn = open_rw(db_path)?;
    conn.execute_batch("DROP TABLE IF EXISTS findings; DROP TABLE IF EXISTS f;")
        .map_err(be)?;
    conn.execute_batch(SCHEMA).map_err(be)?;
    let (superseded, tombstoned) = lifecycle_sets(findings);
    let tx = conn.unchecked_transaction().map_err(be)?;
    for f in findings {
        let (sup, tomb) = flags(f, &superseded, &tombstoned);
        tx.execute(
            "INSERT OR REPLACE INTO findings(id,text,superseded,tombstoned) VALUES(?,?,?,?)",
            rusqlite::params![f.id, f.text, sup, tomb],
        )
        .map_err(be)?;
        tx.execute(
            "INSERT INTO f(id,text) VALUES(?,?)",
            rusqlite::params![f.id, f.text],
        )
        .map_err(be)?;
    }
    tx.commit().map_err(be)?;
    Ok(())
}

/// Insert a newly-created finding and apply its supersede/tombstone tags to
/// the targets. Write path only (serialized by the store's write lock).
pub(super) fn insert(db_path: &Path, finding: &Finding) -> Result<(), DbError> {
    let conn = open_rw(db_path)?;
    let self_tomb = i64::from(carries_lifecycle_tag(finding, "tombstone:"));
    conn.execute(
        "INSERT OR REPLACE INTO findings(id,text,superseded,tombstoned) VALUES(?,?,0,?)",
        rusqlite::params![finding.id, finding.text, self_tomb],
    )
    .map_err(be)?;
    conn.execute(
        "INSERT INTO f(id,text) VALUES(?,?)",
        rusqlite::params![finding.id, finding.text],
    )
    .map_err(be)?;
    for tag in &finding.tags {
        if let Some(target) = tag.strip_prefix("supersedes:") {
            conn.execute(
                "UPDATE findings SET superseded=1 WHERE id=?",
                rusqlite::params![target],
            )
            .map_err(be)?;
        }
        if let Some(target) = tag.strip_prefix("tombstone:") {
            conn.execute(
                "UPDATE findings SET tombstoned=1 WHERE id=?",
                rusqlite::params![target],
            )
            .map_err(be)?;
        }
    }
    Ok(())
}

/// Bounded BM25 candidates from the persistent index, lifecycle-filtered.
/// Read path: opens read-only, builds nothing. Returns `(id, score)` where
/// score is `-bm25` (higher is better). Empty when the query has no term.
#[expect(
    clippy::cast_possible_wrap,
    reason = "bounded small positive pool count bound to a SQLite LIMIT parameter"
)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "BM25 score narrowed to the f32 hit score; magnitude is small and precision loss is immaterial to ranking"
)]
pub(super) fn search_lexical(
    db_path: &Path,
    query: &str,
    pool: usize,
) -> Result<Vec<(String, f32)>, DbError> {
    let Some(expr) = match_expr(query) else {
        return Ok(Vec::new());
    };
    let conn =
        Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(be)?;
    let mut stmt = conn
        .prepare(
            "SELECT f.id, bm25(f) FROM f JOIN findings d ON d.id = f.id \
             WHERE f MATCH ?1 AND d.superseded = 0 AND d.tombstoned = 0 \
             ORDER BY bm25(f) LIMIT ?2",
        )
        .map_err(be)?;
    let rows = stmt
        .query_map(rusqlite::params![expr, pool as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
        })
        .map_err(be)?;
    let mut out = Vec::new();
    for row in rows {
        let (id, bm25) = row.map_err(be)?;
        out.push((id, -bm25 as f32)); // bm25 lower-is-better → negate
    }
    Ok(out)
}

/// `(id, text)` for every non-superseded, non-tombstoned finding — the
/// candidate set the vector arm scores against. Read path: opens read-only,
/// builds nothing. (Reads a persisted table; an ANN vector index that bounds
/// this scan is a future refinement.)
pub(super) fn live_records(db_path: &Path) -> Result<Vec<(String, String)>, DbError> {
    let conn =
        Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(be)?;
    let mut stmt = conn
        .prepare("SELECT id, text FROM findings WHERE superseded = 0 AND tombstoned = 0")
        .map_err(be)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(be)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(be)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{insert, live_records, rebuild, search_lexical};
    use crate::api::types::Finding;

    fn finding(id: &str, text: &str, tags: &[&str]) -> Finding {
        Finding {
            id: id.to_owned(),
            text: text.to_owned(),
            embedding: None,
            tags: tags.iter().map(|s| (*s).to_owned()).collect(),
            parent_ids: vec![],
            created_at: time::OffsetDateTime::from_unix_timestamp(1_700_000_000)
                .unwrap()
                .into(),
        }
    }

    #[test]
    fn rebuild_then_lexical_search_finds_by_term() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("findings.db");
        rebuild(
            &db,
            &[
                finding("fnd_a", "cancel an order and refund the buyer", &[]),
                finding("fnd_b", "render the dashboard chart", &[]),
            ],
        )
        .unwrap();
        let hits = search_lexical(&db, "refund order", 64).unwrap();
        let ids: Vec<&str> = hits.iter().map(|(id, _)| id.as_str()).collect();
        assert!(ids.contains(&"fnd_a"), "term match should surface fnd_a");
        assert!(!ids.contains(&"fnd_b"), "non-matching finding excluded");
    }

    #[test]
    fn supersede_and_tombstone_are_filtered() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = dir.path().join("findings.db");
        rebuild(
            &db,
            &[finding("fnd_old", "the original rule about widgets", &[])],
        )
        .unwrap();
        // A correction supersedes the original.
        insert(
            &db,
            &finding(
                "fnd_new",
                "the revised rule about widgets",
                &["supersedes:fnd_old"],
            ),
        )
        .unwrap();
        let live: Vec<String> = live_records(&db)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(live.contains(&"fnd_new".to_owned()));
        assert!(
            !live.contains(&"fnd_old".to_owned()),
            "superseded excluded from live set"
        );
        // Lexical search also excludes the superseded original.
        let hits = search_lexical(&db, "widgets rule", 64).unwrap();
        let ids: Vec<&str> = hits.iter().map(|(id, _)| id.as_str()).collect();
        assert!(ids.contains(&"fnd_new"));
        assert!(!ids.contains(&"fnd_old"));
    }
}
