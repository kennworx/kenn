//! Snapshot-level jobs: the embedding passes (`reembed`, `embed_pending`)
//! and `stage_findings_for_publish`.
//!
//! On the `SQLite` backend the search store's `vec0` table lives inside
//! `vector.db`, so an embedding pass fills it **in place** — no rebuild
//! and no directory swap (the Lance backend rebuilt + atomically swapped a
//! sibling dataset). `kenn index` reconciles committed sidecar vectors into
//! `vec0` at finalize; these passes embed whatever symbols the sidecar did
//! not already cover, insert the vectors into `vec0`, and append them to the
//! committed sidecar so the next index reuses them.
//!
//! Findings are records-based, so `stage_findings_for_publish` only fences
//! the publish flip with the findings lock — no per-run mirror is built.

use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::Connection;

use crate::api::types::DbError;
use crate::embed::sidecar;

use super::findings;
use super::names;

/// `EmbeddingGemma` full dimension — matches the `vec0 float[768]` schema.
const EMBED_DIM: u32 = 768;

/// What an embedding pass did.
pub struct ReembedReport {
    /// Vectors written this pass.
    pub vectors: usize,
    /// Wall-clock seconds spent purely in the embedding producer.
    pub embed_seconds: f64,
    /// Whether an embedder was available for this pass. `false` only when no
    /// model is configured (lexical-only) — lets a caller tell "embeddings are
    /// disabled" from "nothing left to embed" (both write zero vectors).
    pub embedder_available: bool,
}

impl ReembedReport {
    /// A no-op pass with an embedder present (nothing pending, or lock
    /// contended).
    const fn empty() -> Self {
        Self {
            vectors: 0,
            embed_seconds: 0.0,
            embedder_available: true,
        }
    }

    /// A no-op pass because no embedder is configured (lexical-only). Distinct
    /// from [`empty`](Self::empty) so callers can report `disabled` vs `ready`.
    const fn disabled() -> Self {
        Self {
            vectors: 0,
            embed_seconds: 0.0,
            embedder_available: false,
        }
    }
}

/// Whether a pass re-embeds the whole corpus or only the symbols that are
/// still missing a vector.
#[derive(Clone, Copy)]
enum EmbedMode {
    /// Fill only `knowledge` rows that have no `vec0` entry yet.
    Pending,
    /// Re-embed every row (clears `vec0` first) — the `kenn update` pass.
    Full,
}

/// Resolve the snapshot directory matching the workspace's staleness key —
/// via `decide_startup_state`, not by following `live`, so under a derived
/// store shared across branches this targets *this* workspace's snapshot.
/// Errors when no snapshot matches or it carries no `vector.db`.
fn live_snapshot_dir(
    layout: &crate::layout::Layout,
    git_aware_skip: bool,
    config_sig: u64,
) -> Result<PathBuf, DbError> {
    let store = crate::layout::Store::open(layout.clone())
        .map_err(|e| DbError::Backend(format!("open store: {e}")))?;
    let live = match crate::lifecycle::decide_startup_state(
        &store,
        layout.source_root(),
        git_aware_skip,
        config_sig,
    ) {
        crate::lifecycle::StartupDecision::Skip { live } => live,
        crate::lifecycle::StartupDecision::Reindex { .. } => {
            return Err(DbError::Backend(
                "no snapshot matches the workspace — run `kenn index` first".to_owned(),
            ));
        }
    };
    if !live.join(names::VECTOR_DB).is_file() {
        return Err(DbError::Backend(
            "snapshot has no knowledge store — run `kenn index` first".to_owned(),
        ));
    }
    Ok(live)
}

/// Re-embed the whole corpus — the `kenn update` pass. Clears `vec0` and
/// embeds every searchable row. The model id comes from the global
/// config; the sidecar generation dir it selects is model-consistent by
/// construction.
pub async fn reembed(
    layout: &crate::layout::Layout,
    git_aware_skip: bool,
    config_sig: u64,
    embedder: &kenn_embed::SharedEmbedder,
) -> Result<ReembedReport, DbError> {
    // The configured model keys the generation dir; a model change simply
    // targets a new generation (the old one stays intact — switching back
    // reuses it with zero re-embeds).
    let model_id = sidecar::current_model_id();
    let result = run_embed_pass(
        layout,
        git_aware_skip,
        config_sig,
        &model_id,
        embedder,
        EmbedMode::Full,
    )
    .await;
    persist_embed_health(layout, &result);
    result
}

/// The incremental background embed job: embed the rows `kenn index` left
/// without a vector (those not covered by the committed sidecar), insert
/// them into `vec0`, and append them to the sidecar.
///
/// **Coordination.** A per-snapshot `flock` at `runs/{id}/embed.lock`
/// serializes embed against embed for the *same* run. A contended lock
/// means another process is already embedding that run — this call returns
/// a zero report rather than running a redundant pass. The lock file is
/// removed naturally when `lifecycle::gc()` evicts the run.
pub async fn embed_pending(
    layout: &crate::layout::Layout,
    git_aware_skip: bool,
    config_sig: u64,
    model_id: &str,
    embedder: &kenn_embed::SharedEmbedder,
) -> Result<ReembedReport, DbError> {
    let result = run_embed_pass(
        layout,
        git_aware_skip,
        config_sig,
        model_id,
        embedder,
        EmbedMode::Pending,
    )
    .await;
    persist_embed_health(layout, &result);
    result
}

/// Path of the local (non-committed) marker recording the last embed pass's
/// backend failure, read by `kenn status` / `get_index_status` to surface a
/// `degraded` embedder. Lives under `derived_root`, never the git-tracked
/// vectors dir.
fn embed_error_path(layout: &crate::layout::Layout) -> PathBuf {
    layout.derived_root().join("embed_error")
}

/// Persist embed health after a pass. A backend embed failure (e.g. the macOS
/// fork+Metal bug) writes its message so the status surfaces can report
/// `degraded`; any success clears the marker. Best-effort — a marker IO failure
/// is irrelevant next to the pass outcome. A non-backend error leaves the marker
/// untouched (it is not an embedder-degradation signal).
fn persist_embed_health(layout: &crate::layout::Layout, result: &Result<ReembedReport, DbError>) {
    let marker = embed_error_path(layout);
    match result {
        Err(DbError::Backend(msg)) => drop(std::fs::write(&marker, msg)),
        Ok(_) => drop(std::fs::remove_file(&marker)),
        Err(_) => {}
    }
}

/// The last embed pass's backend error, if the embedder is currently
/// **degraded** — a model is configured but embedding failed. `None` when
/// embedding last succeeded, is disabled (no model), or was never run.
#[must_use]
pub fn read_embed_error(layout: &crate::layout::Layout) -> Option<String> {
    let contents = std::fs::read_to_string(embed_error_path(layout)).ok()?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Shared body of [`reembed`] / [`embed_pending`]: resolve the snapshot,
/// fence with the per-snapshot embed lock, pin it against GC, fill `vec0`
/// in place, and append the new vectors to the committed sidecar.
async fn run_embed_pass(
    layout: &crate::layout::Layout,
    git_aware_skip: bool,
    config_sig: u64,
    model_id: &str,
    embedder: &kenn_embed::SharedEmbedder,
    mode: EmbedMode,
) -> Result<ReembedReport, DbError> {
    let snapshot_dir = live_snapshot_dir(layout, git_aware_skip, config_sig)?;
    let vectors_dir = sidecar::code_generation_dir(layout, model_id);
    let snapshot_id = snapshot_dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| {
            DbError::Backend(format!(
                "snapshot path has no file name: {}",
                snapshot_dir.display()
            ))
        })?
        .to_owned();

    // Per-snapshot embed coordination — co-located with the `vector.db`
    // it protects against concurrent `vec0` fills. A contended lock means a
    // peer is already embedding this snapshot; skip rather than duplicate.
    let lock_path = snapshot_dir.join("embed.lock");
    let lock_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(DbError::Io)?;
    if fs2::FileExt::try_lock_exclusive(&lock_file).is_err() {
        return Ok(ReembedReport::empty());
    }

    // Pin the snapshot against GC for the duration of this embed: the embed
    // lock is per-run, not the store's `index.lock`, so a concurrent reindex
    // could publish + the prior pin release + a GC sweep evict this snapshot
    // mid-write. The reader registry pin closes that gap.
    let store_handle = crate::layout::Store::open(layout.clone())
        .map_err(|e| DbError::Backend(format!("opening store for embed pin: {e}")))?;
    let _embed_pin = crate::readers::register_reader(&store_handle, &snapshot_dir)
        .map_err(|e| DbError::Backend(format!("pinning snapshot for embed: {e}")))?;

    // Findings embeddings ride the same per-snapshot lock — independent of
    // the code-graph embed below, so they run even when the code store has
    // nothing pending. Best-effort: a findings embed failure must not abort
    // the code embed pass.
    if let Err(e) = findings::embed_findings(layout, model_id, embedder).await {
        tracing::warn!(error = %e, "findings embed pass failed");
    }

    // The generation dir is keyed by (model, dim, quant, recipe), so a model
    // or recipe change targets a fresh dir — no wipe, no mixing, and the old
    // generation stays reusable. Stamp the manifest (defense-in-depth for the
    // read-side gate) before embedding, so the early `pending.is_empty()`
    // return still leaves a consistent manifest.
    if sidecar::Manifest::read(&vectors_dir)?.is_none() {
        sidecar::Manifest::current(model_id.to_owned(), EMBED_DIM, sidecar::CODE_TEXT_RECIPE)
            .write(&vectors_dir)?;
    }

    // Lazy vector-cache GC at the start of an embed pass — the one moment
    // both index paths (CLI and workflow/MCP) already funnel through.
    // Best-effort: a GC failure must not abort the embed.
    if let Err(e) = sidecar::gc_vector_cache(layout, model_id, layout.vectors_cache_cap_mb()) {
        tracing::warn!(target: "kenn_store::embed", error = %e, "vector-cache GC failed");
    }

    super::sqlite::ensure_vec_extension();
    let conn = Connection::open(snapshot_dir.join(names::VECTOR_DB)).map_err(be)?;
    conn.busy_timeout(std::time::Duration::from_secs(30))
        .map_err(be)?;

    // Rows to embed — `Full` re-embeds every name row, `Pending` only those
    // without a vector yet. `name` rows are the embeddable unit (one per
    // symbol); the `fingerprint` column is the sidecar key. We scan and
    // embed *before* touching `vec0`, so a disabled embedder or an embed
    // error never wipes existing vectors (the `Full` clear happens in the
    // same transaction as the re-insert, below).
    let pending = scan_rows(&conn, mode)?;
    if pending.is_empty() {
        return Ok(ReembedReport::empty());
    }

    let texts: Vec<&str> = pending.iter().map(|r| r.text.as_str()).collect();
    let started = Instant::now();
    let Some(vectors) = embedder.embed_block_until_ready(&texts).await? else {
        // Embedding is disabled (no model) — degrade to lexical-only. Signal it
        // distinctly so the caller can report "disabled" vs "ready".
        return Ok(ReembedReport::disabled());
    };
    let embed_seconds = started.elapsed().as_secs_f64();
    if vectors.len() != pending.len() {
        return Err(DbError::Backend(format!(
            "embedder returned {} vectors for {} inputs",
            vectors.len(),
            pending.len()
        )));
    }

    let new_entries = insert_vectors(&conn, &pending, vectors, mode)?;

    // KVS2 write protocol: tmp + atomic rename, content-addressed filename.
    // Dev-local `seg-` prefix; CI's `--repack` promotes segs to packs later.
    let tmp_dir = layout.writer_tmp_dir(&snapshot_id);
    sidecar::append_vectors(
        &vectors_dir,
        &tmp_dir,
        sidecar::WriterPrefix::Seg,
        EMBED_DIM,
        &new_entries,
    )?;
    Ok(ReembedReport {
        vectors: new_entries.len(),
        embed_seconds,
        embedder_available: true,
    })
}

/// One `knowledge` row that needs a vector: its `vec0` rowid, the text to
/// embed, and the sidecar fingerprint (the `fingerprint` column).
struct Unembedded {
    rowid: i64,
    text: String,
    fingerprint: u64,
}

/// Scan `knowledge` for the name rows to embed — one embeddable unit per
/// symbol. `Pending` skips rows that already have a `vec0` entry; `Full`
/// returns every name row (the existing `vec0` is cleared atomically at
/// insert time). The embeddable text is the symbol's **doc prose only** (the
/// `doc` recipe), reconstructed from the joined `doc` row; it matches the text
/// `finalize` fingerprinted. Undocumented symbols have empty text and are
/// skipped — they get no vector and stay findable via the lexical arms.
fn scan_rows(conn: &Connection, mode: EmbedMode) -> Result<Vec<Unembedded>, DbError> {
    let filter = match mode {
        EmbedMode::Full => "",
        EmbedMode::Pending => "AND n.rowid NOT IN (SELECT rowid FROM vec_knowledge)",
    };
    // Join AT MOST ONE doc row per name (the lowest-rowid one — the first
    // `symbol_docs` entry, matching `finalize`'s `… LIMIT 1` doc choice). A
    // symbol id can own several doc rows (e.g. a C# partial type documented in
    // multiple files); a plain `d.id = n.id` join would then fan the name row
    // out into duplicate pending units and `insert_vectors` would hit the
    // `vec_knowledge` rowid UNIQUE constraint.
    let sql = format!(
        "SELECT n.rowid, COALESCE(d.doc_text, ''), n.fingerprint \
         FROM knowledge n \
         LEFT JOIN knowledge d ON d.rowid = ( \
             SELECT MIN(dd.rowid) FROM knowledge dd \
             WHERE dd.id = n.id AND dd.row_kind = 'doc') \
         WHERE n.row_kind = 'name' {filter}"
    );
    let mut stmt = conn.prepare(&sql).map_err(be)?;
    let rows = stmt
        .query_map([], |r| {
            // Doc-only recipe: the vector text is the doc prose alone.
            let text: String = r.get(1)?;
            #[expect(
                clippy::cast_sign_loss,
                reason = "fingerprint is a u64 xxh3 hash stored as its i64 bit-pattern; the cast restores the original bits"
            )]
            let fingerprint = r.get::<_, i64>(2)? as u64;
            Ok(Unembedded {
                rowid: r.get(0)?,
                text,
                fingerprint,
            })
        })
        .map_err(be)?;
    let mut out = Vec::new();
    for row in rows {
        let row = row.map_err(be)?;
        if !row.text.is_empty() {
            out.push(row);
        }
    }
    Ok(out)
}

/// Insert `vectors` into `vec0` (keyed by each row's `vec0` rowid) inside a
/// single transaction, and return the `(fingerprint, vector)` pairs to
/// append to the sidecar. For `Full`, the existing `vec0` is cleared in the
/// same transaction — so a re-embed only ever replaces vectors atomically,
/// never leaves `vec0` empty on a failure.
fn insert_vectors(
    conn: &Connection,
    pending: &[Unembedded],
    vectors: Vec<Vec<f32>>,
    mode: EmbedMode,
) -> Result<Vec<(u64, Vec<f32>)>, DbError> {
    let tx = conn.unchecked_transaction().map_err(be)?;
    if matches!(mode, EmbedMode::Full) {
        tx.execute("DELETE FROM vec_knowledge", []).map_err(be)?;
    }
    let mut new_entries = Vec::with_capacity(pending.len());
    {
        let mut ins = tx
            .prepare_cached("INSERT INTO vec_knowledge(rowid, embedding) VALUES(?,?)")
            .map_err(be)?;
        for (row, vector) in pending.iter().zip(vectors) {
            let bytes: Vec<u8> = vector.iter().flat_map(|f| f.to_le_bytes()).collect();
            ins.execute(rusqlite::params![row.rowid, bytes])
                .map_err(be)?;
            new_entries.push((row.fingerprint, vector));
        }
    }
    tx.commit().map_err(be)?;
    Ok(new_entries)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as a map_err fn pointer, which passes the error by value"
)]
fn be(e: rusqlite::Error) -> DbError {
    DbError::Backend(format!("embed sqlite: {e}"))
}

/// Fence the publish flip with the findings lock. Findings are records-based,
/// so there is no per-run mirror to build — the returned lock guard MUST be
/// held across the caller's `live` flip and dropped immediately after.
pub async fn stage_findings_for_publish(
    layout: &crate::layout::Layout,
    _run_dir: &Path,
) -> Result<std::fs::File, DbError> {
    findings::acquire_findings_publish_lock(layout)
}

#[cfg(test)]
mod tests {
    use super::{persist_embed_health, read_embed_error, scan_rows, EmbedMode, ReembedReport};
    use crate::api::types::DbError;
    use crate::layout::Layout;
    use rusqlite::Connection;

    /// A backend embed failure writes the `embed_error` marker (surfacing a
    /// `degraded` embedder to `kenn status` / `get_index_status`); a subsequent
    /// successful pass clears it; a non-backend error leaves it untouched.
    #[test]
    fn embed_error_marker_round_trips() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let layout = Layout::default_for(dir.path());
        std::fs::create_dir_all(layout.derived_root()).expect("mk derived_root");

        assert_eq!(read_embed_error(&layout), None, "clean start");

        persist_embed_health(&layout, &Err(DbError::Backend("fork+metal boom".into())));
        assert_eq!(
            read_embed_error(&layout).as_deref(),
            Some("fork+metal boom"),
            "backend failure recorded"
        );

        persist_embed_health(&layout, &Ok(ReembedReport::empty()));
        assert_eq!(read_embed_error(&layout), None, "success clears the marker");

        // A non-backend error must not fabricate a degraded state.
        persist_embed_health(
            &layout,
            &Err(DbError::Io(std::io::Error::other("unrelated"))),
        );
        assert_eq!(
            read_embed_error(&layout),
            None,
            "non-backend error is inert"
        );
    }

    fn knowledge_conn() -> Connection {
        // Only the `knowledge` table is needed — `scan_rows` in `Full` mode
        // never references `vec_knowledge`, so no vec0 extension is required.
        let c = Connection::open_in_memory().expect("open in-memory");
        c.execute_batch(
            "CREATE TABLE knowledge (\
               rowid INTEGER PRIMARY KEY, embed_key TEXT NOT NULL, id INTEGER NOT NULL, \
               row_kind TEXT NOT NULL, language TEXT NOT NULL, pub_id TEXT NOT NULL, \
               path TEXT, name TEXT, kind TEXT, name_text TEXT, doc_text TEXT, \
               fingerprint INTEGER NOT NULL);",
        )
        .expect("knowledge table");
        c
    }

    /// A symbol id that owns more than one `doc` row (e.g. a C# partial type
    /// documented in several files) must still yield exactly ONE pending
    /// embedding unit for its name row — not one per doc row. The fan-out
    /// regression made `insert_vectors` insert the same `vec_knowledge` rowid
    /// twice and fail the primary-key UNIQUE constraint.
    #[test]
    fn scan_rows_keeps_one_unit_when_a_symbol_has_multiple_doc_rows() {
        let c = knowledge_conn();
        c.execute(
            "INSERT INTO knowledge(rowid,embed_key,id,row_kind,language,pub_id,name_text,fingerprint) \
             VALUES(1,'k',7,'name','cs','p','sig text',0)",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO knowledge(rowid,embed_key,id,row_kind,language,pub_id,doc_text,fingerprint) \
             VALUES(2,'k',7,'doc','cs','p','first doc',0)",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO knowledge(rowid,embed_key,id,row_kind,language,pub_id,doc_text,fingerprint) \
             VALUES(3,'k',7,'doc','cs','p','second doc',0)",
            [],
        )
        .unwrap();

        let rows = scan_rows(&c, EmbedMode::Full).expect("scan");
        assert_eq!(
            rows.len(),
            1,
            "expected one unit per name, got {}",
            rows.len()
        );
        let unit = rows.first().expect("one unit");
        assert_eq!(unit.rowid, 1);
        // Doc-only recipe: the embed text is the first (min-rowid) doc alone,
        // matching finalize's choice — not the signature.
        assert_eq!(unit.text, "first doc");
    }
}
