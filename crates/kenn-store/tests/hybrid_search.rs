//! Real-model hybrid-search integration tests for the
//! embedding-producer change:
//!
//! - 3.2 — a flushed finding carries a usable embedding (proved by paraphrase retrieval).
//! - 5.3 — a paraphrase query with no shared terms retrieves the right code symbol and finding.
//! - 6.2 — `store_finding` surfaces a semantically close prior finding.
//!
//! These need the `EmbeddingGemma` GGUF. Each test resolves the model
//! from `KENN_EMBED_MODEL` or the standard cache and self-skips when no
//! model is available, so a plain `cargo test` stays green offline.

use kenn_model::{FileRecord, Kind, Language, PackageRecord, SymbolDocsRecord, SymbolRecord};
use kenn_store::api::{Reader, WriteBatch};
use kenn_store::{
    lifecycle, open_reader, open_writer, CodeNodeResolver, FindingsStore, Layout, Store,
    WriterOptions,
};
use serial_test::file_serial;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

// ── model resolution / skip ─────────────────────────────────────────

/// Resolve a usable GGUF — explicit `KENN_EMBED_MODEL_PATH` override,
/// then the standard cache. `KENN_EMBED_MODEL` is reserved for the
/// model *id string* sent in `/v1/embeddings` requests and stamped in
/// the manifest; the filesystem path is `KENN_EMBED_MODEL_PATH`.
fn model_path() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("KENN_EMBED_MODEL_PATH") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }
    let home = std::env::var("HOME").ok()?;
    let cached = PathBuf::from(home).join(".cache/kenn/models/embeddinggemma-300M-Q8_0.gguf");
    cached.is_file().then_some(cached)
}

/// Enable embedding against a resolved model. `false` → the test skips.
///
/// Sets `KENN_EMBED_MODEL_PATH` (the filesystem path), NOT
/// `KENN_EMBED_MODEL` — the latter is the model *id string* sent to
/// `/v1/embeddings` and stamped in the manifest, which must stay as
/// the short name `embeddinggemma-300M` so the test's local kenn
/// server (configured with the short name) accepts the request.
fn enable_model() -> bool {
    if let Some(path) = model_path() {
        // Point the in-process embedder at the GGUF file. The model
        // *id string* used in API requests defaults to the short name
        // (`embeddinggemma-300M`) via kenn-config; don't override it
        // with the path or the embedding daemon will reject the call
        // with `model_not_found`.
        std::env::set_var("KENN_EMBED_MODEL_PATH", path);
        // On macOS the auto-spawned kenn server daemon returns empty
        // embeddings (fork+Metal bug), so force the in-process backend.
        // Tests are `#[file_serial]`, so a single resident model is shared
        // sequentially across the suite.
        if cfg!(target_os = "macos") {
            std::env::set_var("KENN_EMBED_IN_PROCESS", "1");
        }
        kenn_store::init_shared_embedder(kenn_config::GlobalConfig::default());
        true
    } else {
        eprintln!("SKIP: no EmbeddingGemma model resolvable (set KENN_EMBED_MODEL_PATH)");
        false
    }
}

// ── code corpus ─────────────────────────────────────────────────────

/// A trivial resolver — every code-node id counts as live.
struct AllLive;
impl CodeNodeResolver for AllLive {
    fn contains(&self, _id: &str) -> bool {
        true
    }
}

/// Frees the shared embedder's model when a test ends. The bundled
/// llama.cpp aborts at Metal-device teardown if a model is still
/// resident at process exit, so every test releases it on the way out.
struct ReleaseGuard;
impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        kenn_store::release_shared_embedder();
    }
}

fn sym(id: u32, name: &str) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: format!("rust:{name}"),
        language: Language::Rust,
        pkg_id: 1,
        kind: Kind::Function,
        name: name.into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

fn doc(sym_id: u32, sig: &str, doc: &str) -> SymbolDocsRecord {
    SymbolDocsRecord {
        sym_id,
        sig: sig.into(),
        doc: doc.into(),
    }
}

/// Five functions with documenting comments. The doc prose is the only
/// place a paraphrase query can land — names and signatures share no
/// words with the query used below.
fn code_batch() -> WriteBatch {
    WriteBatch {
        packages: vec![PackageRecord {
            id: 1,
            name: "kenn-hybrid-test".into(),
            version: "0.0.0".into(),
            manager: "cargo".into(),
            external: false,
        }],
        files: vec![FileRecord {
            id: 1,
            path: "src/lib.rs".into(),
            language: Language::Rust,
            test: false,
            external: false,
            content_hash: 0,
        }],
        symbols: vec![
            sym(100, "terminate_process"),
            sym(101, "parse_config"),
            sym(102, "compute_checksum"),
            sym(103, "connect_database"),
            sym(104, "format_timestamp"),
        ],
        symbol_docs: vec![
            doc(
                100,
                "fn terminate_process()",
                "stops a running program and releases the resources it held",
            ),
            doc(
                101,
                "fn parse_config()",
                "reads a configuration file and returns the parsed settings",
            ),
            doc(
                102,
                "fn compute_checksum()",
                "calculates a hash digest over the bytes of a buffer",
            ),
            doc(
                103,
                "fn connect_database()",
                "opens a network connection to the database server",
            ),
            doc(
                104,
                "fn format_timestamp()",
                "renders a unix time value as a human-readable date string",
            ),
        ],
        file_docs: vec![],
        defs: vec![],
        edges: vec![],
    }
}

/// Index the code corpus through the real lifecycle and publish a live
/// snapshot — `reembed` / `embed_pending` resolve the knowledge store
/// from `live`, so the corpus must be a published snapshot. When
/// `vectors_dir` is set the writer reconciles against that sidecar.
async fn build_code_corpus(dir: &Path, vectors_dir: Option<PathBuf>) {
    let store = Store::open_default(dir).expect("store");
    let handle = lifecycle::begin_indexing(&store).expect("begin_indexing");
    // Reconciliation is model-gated; the fixtures embed with the default model.
    let vectors_model_id = vectors_dir
        .is_some()
        .then(|| kenn_config::EmbeddingsConfig::default().model);
    let writer = open_writer(
        handle.run_dir(),
        WriterOptions {
            vectors_dir,
            vectors_model_id,
            ..WriterOptions::default()
        },
    )
    .await
    .expect("open_writer");
    writer
        .write_batch(&code_batch())
        .await
        .expect("write_batch");
    writer.finalize().await.expect("finalize");
    drop(writer);
    // KVS2 / D1 — publish refuses without `meta.json` completion stamp.
    let meta = serde_json::json!({
        "status": "success",
        "schema_version": kenn_store::STORE_SCHEMA_VERSION,
    });
    std::fs::write(
        handle.run_dir().join("meta.json"),
        serde_json::to_vec(&meta).expect("meta serde"),
    )
    .expect("meta");
    handle.publish().expect("publish");
}

// ── 5.3 (code) ──────────────────────────────────────────────────────

/// A paraphrase with no shared terms retrieves the right symbol: the
/// query "shut down an application" shares no word with
/// `terminate_process` or its doc, yet hybrid search surfaces it via
/// the vector arm.
#[tokio::test(flavor = "multi_thread")]
#[file_serial(embedder)]
async fn paraphrase_query_retrieves_code_symbol() {
    let _guard = ReleaseGuard;
    if !enable_model() {
        return;
    }
    let dir = TempDir::new().unwrap();
    build_code_corpus(dir.path(), None).await;
    // Code embedding is a separate pass (`kenn update`) — `kenn index`
    // writes the structural store only. Run it so the vector arm has
    // committed vectors to search.
    kenn_store::reembed(
        &Layout::default_for(dir.path()),
        false,
        0,
        kenn_store::shared_embedder(),
    )
    .await
    .expect("reembed code corpus");
    let live = Store::open_default(dir.path())
        .expect("store")
        .live_target()
        .expect("live snapshot");
    let reader = open_reader(&live).await.expect("open_reader");

    let query = "shut down an application";
    // Pre-embed via the bulk path so transient `Starting`/`Unreachable`
    // states are retried internally — the production search path goes
    // through `db_to_mcp` and the agent retries; the test does the same
    // via `embed_block_until_ready`.
    let query_vec = kenn_store::shared_embedder()
        .embed_block_until_ready(&[query])
        .await
        .expect("embed query")
        .and_then(|mut v| v.drain(..).next());
    let hits = reader
        .search_symbols_blended(query, query_vec.as_deref(), 5, false, false)
        .await
        .expect("blended search");
    let names: Vec<&str> = hits.iter().map(|h| h.symbol.name.as_str()).collect();
    assert!(
        names.contains(&"terminate_process"),
        "paraphrase query surfaced the right symbol via the vector arm; got {names:?}"
    );
}

// ── incremental embed job ───────────────────────────────────────────

/// A full re-embed with an unavailable embedder must not wipe the vectors it
/// cannot replace.
///
/// This is why the `Full` clear lives inside the **first chunk's** insert
/// transaction rather than before the loop (design D2). Chunking made that
/// placement load-bearing: a `DELETE FROM vec_knowledge` hoisted above the loop
/// would run before the first submission tells us the embedder is missing, and
/// a workspace whose embedder simply is not running would lose its vectors.
#[tokio::test(flavor = "multi_thread")]
#[file_serial(embedder)]
async fn a_full_pass_without_an_embedder_keeps_existing_vectors() {
    let _guard = ReleaseGuard;
    if !enable_model() {
        return;
    }
    let dir = TempDir::new().unwrap();
    build_code_corpus(dir.path(), None).await;
    let layout = Layout::default_for(dir.path());
    let model = kenn_config::EmbeddingsConfig::default().model.clone();
    let vectors_dir = kenn_store::code_generation_dir(&layout, &model);

    // Populate: five rows embedded, segments on disk.
    let seeded =
        kenn_store::embed_pending(&layout, false, 0, &model, kenn_store::shared_embedder())
            .await
            .expect("seed embed");
    assert_eq!(seeded.vectors, 5);
    let before = std::fs::read_dir(&vectors_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("seg-"))
        .count();
    assert!(before >= 1, "seed produced no segments");

    // Take the model away and run a FULL pass, which is the mode that clears.
    kenn_store::release_shared_embedder();
    std::env::set_var(
        "KENN_EMBED_MODEL_PATH",
        dir.path().join("no-such-model.gguf"),
    );
    let report = kenn_store::reembed(&layout, false, 0, kenn_store::shared_embedder()).await;
    std::env::remove_var("KENN_EMBED_MODEL_PATH");

    // A missing model surfaces as `Err(Backend("no embedder available"))`
    // rather than the `disabled` degradation, which is reserved for embedding
    // being switched off. Either way the invariant is the same and it is the
    // one D2 turns on: the pass failed, so it must not have cleared anything.
    assert!(
        report.as_ref().err().is_some() || !report.as_ref().unwrap().embedder_available,
        "expected a full pass with no model to fail or degrade, got a success"
    );
    // The vectors at risk are the `vec_knowledge` rows, not the sidecar
    // segments on disk — a `DELETE FROM vec_knowledge` leaves every seg- file
    // untouched, so counting files cannot tell the two apart (it did not: the
    // first version of this test survived the mutation it exists to catch).
    // The distinguishing observable is what a *subsequent* incremental pass
    // finds pending: nothing, if the rows survived; all five, if they were
    // cleared.
    std::env::set_var("KENN_EMBED_MODEL_PATH", model_path().expect("model"));
    kenn_store::release_shared_embedder();
    let after = kenn_store::embed_pending(&layout, false, 0, &model, kenn_store::shared_embedder())
        .await
        .expect("incremental pass after the failed full pass");
    assert_eq!(
        after.vectors, 0,
        "a full pass that could not embed must leave the existing vectors in \
         place — {} rows came back pending, so they were cleared before the \
         embedder was found to be missing",
        after.vectors
    );
    let _ = before;
}

/// The pass chunks its scan instead of holding the corpus.
///
/// Before `bounded-embed-pass` it issued one `scan_rows` and one
/// `embed_block_until_ready` for the whole match set, so texts, vectors and
/// sidecar entries were all corpus-sized at once — ~3 KB per row, ~93 MB on
/// kenn's own repo and linear in the corpus after that. The observable
/// consequence of chunking is that a corpus larger than `batch_size` appends
/// more than one segment: entries are appended per chunk precisely so they are
/// not accumulated.
///
/// `KENN_EMBED_BATCH_SIZE` makes this testable without embedding 257 rows.
/// `std::env::set_var` is safe on edition 2021, and `file_serial(embedder)`
/// already serializes every test that touches the embedder.
#[tokio::test(flavor = "multi_thread")]
#[file_serial(embedder)]
async fn the_embed_pass_chunks_its_scan() {
    let _guard = ReleaseGuard;
    if !enable_model() {
        return;
    }
    let dir = TempDir::new().unwrap();
    build_code_corpus(dir.path(), None).await;
    let vectors_dir = kenn_store::code_generation_dir(
        &Layout::default_for(dir.path()),
        &kenn_config::EmbeddingsConfig::default().model,
    );

    // Five doc'd rows, chunked two at a time → three chunks.
    std::env::set_var("KENN_EMBED_BATCH_SIZE", "2");
    let report = kenn_store::embed_pending(
        &Layout::default_for(dir.path()),
        false,
        0,
        &kenn_config::EmbeddingsConfig::default().model,
        kenn_store::shared_embedder(),
    )
    .await;
    std::env::remove_var("KENN_EMBED_BATCH_SIZE");
    let report = report.expect("embed_pending");

    assert_eq!(
        report.vectors, 5,
        "chunking must not change what is embedded"
    );
    let segments = std::fs::read_dir(&vectors_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("seg-"))
        .count();
    assert!(
        segments > 1,
        "expected the pass to append per chunk (>1 seg- file for 5 rows at \
         batch_size 2), got {segments} — a single segment means the whole \
         corpus was embedded and accumulated in one shot"
    );
}

/// The incremental embed job (`embed_pending`) embeds only the symbols
/// left null, appends one sidecar segment plus a manifest, and is a
/// clean no-op on a second run (incremental-embedding 3.1 / 3.6).
#[tokio::test(flavor = "multi_thread")]
#[file_serial(embedder)]
async fn embed_pending_fills_nulls_then_is_idempotent() {
    let _guard = ReleaseGuard;
    if !enable_model() {
        return;
    }
    let dir = TempDir::new().unwrap();
    // Built with no `vectors_dir` → no index-time reconciliation → the
    // structural store has five null name rows.
    build_code_corpus(dir.path(), None).await;
    // The sidecar's current generation dir (model-keyed) under `.kenn/vectors/`.
    let vectors_dir = kenn_store::code_generation_dir(
        &Layout::default_for(dir.path()),
        &kenn_config::EmbeddingsConfig::default().model,
    );

    // First run embeds all five doc'd symbols.
    let first = kenn_store::embed_pending(
        &Layout::default_for(dir.path()),
        false,
        0,
        &kenn_config::EmbeddingsConfig::default().model,
        kenn_store::shared_embedder(),
    )
    .await
    .expect("embed_pending");
    assert_eq!(first.vectors, 5, "every null name row embedded once");
    assert!(
        vectors_dir.join("manifest.toml").is_file(),
        "manifest written on the first segment"
    );
    let segments = std::fs::read_dir(&vectors_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("seg-"))
        .count();
    assert_eq!(segments, 1, "exactly one seg- file appended");

    // Second run: every row now carries a vector → nothing pending.
    let second = kenn_store::embed_pending(
        &Layout::default_for(dir.path()),
        false,
        0,
        &kenn_config::EmbeddingsConfig::default().model,
        kenn_store::shared_embedder(),
    )
    .await
    .expect("embed_pending again");
    assert_eq!(second.vectors, 0, "idempotent — no re-embedding");
}

/// A fresh worktree — a separate store directory — reconciles against
/// the committed sidecar: every unchanged symbol reuses its vector at
/// index time, so the embed job finds nothing to embed
/// (incremental-embedding 6.2).
#[tokio::test(flavor = "multi_thread")]
#[file_serial(embedder)]
async fn fresh_worktree_reuses_committed_vectors() {
    let _guard = ReleaseGuard;
    if !enable_model() {
        return;
    }

    // Worktree A: build and embed — this populates the committed sidecar.
    let a = TempDir::new().unwrap();
    // The sidecar's current generation dir (model-keyed) under `.kenn/vectors/`.
    let vectors = kenn_store::code_generation_dir(
        &Layout::default_for(a.path()),
        &kenn_config::EmbeddingsConfig::default().model,
    );
    build_code_corpus(a.path(), None).await;
    let first = kenn_store::embed_pending(
        &Layout::default_for(a.path()),
        false,
        0,
        &kenn_config::EmbeddingsConfig::default().model,
        kenn_store::shared_embedder(),
    )
    .await
    .expect("embed worktree A");
    assert_eq!(first.vectors, 5, "worktree A embeds the whole corpus");

    // Worktree B: a separate store, reconciling against A's sidecar.
    let b = TempDir::new().unwrap();
    build_code_corpus(b.path(), Some(vectors.clone())).await;
    // Index-time reconciliation filled every row from the sidecar, so
    // the embed job has nothing left to do.
    let second = kenn_store::embed_pending(
        &Layout::default_for(b.path()),
        false,
        0,
        &kenn_config::EmbeddingsConfig::default().model,
        kenn_store::shared_embedder(),
    )
    .await
    .expect("embed worktree B");
    assert_eq!(
        second.vectors, 0,
        "the fresh worktree reused every committed vector — no re-embedding"
    );
}

/// `embed_pending` skips when the per-snapshot embed lock is held by
/// another process (`mcp-background-reindex` Decision 6). Returns a zero
/// report — NOT an error — so cold-start and hot-reload at multiple
/// instances coalesce silently onto the winner's embed run.
/// `index.lock` is no longer taken: a long embed against the live
/// snapshot must not block a `reindex` that publishes a future snapshot.
#[tokio::test(flavor = "multi_thread")]
#[file_serial(embedder)]
async fn embed_pending_skips_when_per_snapshot_lock_held() {
    let dir = TempDir::new().unwrap();
    build_code_corpus(dir.path(), None).await;
    let layout = Layout::default_for(dir.path());

    // Resolve the snapshot dir to locate its embed.lock.
    let store = kenn_store::Store::open(layout.clone()).unwrap();
    let snap = store.live_target().expect("live snapshot");
    let lock_path = snap.join("embed.lock");
    let hog = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    fs2::FileExt::lock_exclusive(&hog).expect("take embed lock");

    // embed_pending sees the contended lock and skips with a zero report.
    let report = kenn_store::embed_pending(
        &layout,
        false,
        0,
        &kenn_config::EmbeddingsConfig::default().model,
        kenn_store::shared_embedder(),
    )
    .await
    .expect("skip-on-contention is not an error");
    assert_eq!(
        report.vectors, 0,
        "embed must skip (not run) when another process holds the per-snapshot lock"
    );
}

// ── 3.2 + 5.3 (findings) ────────────────────────────────────────────

/// A flushed finding is retrievable by a paraphrase query — which is
/// only possible if `flush` populated its `embedding` column (task 3.2)
/// and the vector arm of `search_findings` is live (task 5.3).
#[tokio::test(flavor = "multi_thread")]
#[file_serial(embedder)]
async fn flushed_finding_retrieved_by_paraphrase() {
    let _guard = ReleaseGuard;
    if !enable_model() {
        return;
    }
    let dir = TempDir::new().unwrap();
    // A live run (with a knowledge store) so the findings mirror has a
    // home and `embed_pending` can resolve the run to fill vectors.
    build_code_corpus(dir.path(), None).await;
    let mut store = FindingsStore::open_default(dir.path()).await.unwrap();
    store
        .store_finding(
            "the lexer treats a leading byte order mark as whitespace".to_owned(),
            vec![],
            vec![],
            None,
        )
        .await
        .unwrap();
    // A distractor on an unrelated topic.
    store
        .store_finding(
            "the scheduler runs jobs in priority order".to_owned(),
            vec![],
            vec![],
            None,
        )
        .await
        .unwrap();
    store.flush().await.unwrap();
    // The sync write path appends with a NULL vector; the async embed
    // pass fills it. Run it so the vector arm has vectors to search.
    kenn_store::embed_pending(
        &Layout::default_for(dir.path()),
        false,
        0,
        &kenn_config::EmbeddingsConfig::default().model,
        kenn_store::shared_embedder(),
    )
    .await
    .expect("embed findings vectors");

    let query = "ignoring the unicode signature while tokenizing";
    let query_vec = kenn_store::shared_embedder()
        .embed_block_until_ready(&[query])
        .await
        .expect("embed query")
        .and_then(|mut v| v.drain(..).next());
    let hits = store
        .search_findings(query, query_vec.as_deref(), 5, &AllLive)
        .await
        .unwrap();
    let texts: Vec<&str> = hits.iter().map(|h| h.finding.text.as_str()).collect();
    assert!(
        texts.iter().any(|t| t.contains("byte order mark")),
        "paraphrase query surfaced the BOM finding; got {texts:?}"
    );

    // The finding's vector lives in the committed findings sidecar's
    // model-keyed generation dir under `.kenn/vectors/`. The committed
    // `.kenn/findings/` directory holds only the Markdown records (no
    // nested `vectors/` subtree). The derived store lives under `local/`.
    let findings_dir = dir.path().join(".kenn").join("findings");
    let vectors_dir = kenn_store::findings_generation_dir(
        &Layout::default_for(dir.path()),
        &kenn_config::EmbeddingsConfig::default().model,
    );
    assert!(
        vectors_dir.join("manifest.toml").is_file(),
        "the findings vector sidecar carries a manifest"
    );
    let segments = std::fs::read_dir(&vectors_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("seg-"))
        .count();
    assert!(
        segments >= 1,
        "the flushed finding's vector is a sidecar seg-* file"
    );
    for entry in std::fs::read_dir(&findings_dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // `.tmp/` holds in-flight finding-record writes before atomic
        // rename onto `<id>.md` — local debris, never durable content.
        if name == ".tmp" && entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let is_record = std::path::Path::new(&name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        assert!(
            is_record,
            "committed findings/ holds only Markdown records (or the `.tmp/` write-staging subdir), found {name}"
        );
    }
}

// ── 6.2 ─────────────────────────────────────────────────────────────

/// `store_finding` surfaces a semantically close prior finding without
/// auto-merging it.
#[tokio::test(flavor = "multi_thread")]
#[file_serial(embedder)]
async fn store_finding_surfaces_near_duplicate() {
    let _guard = ReleaseGuard;
    if !enable_model() {
        return;
    }
    let dir = TempDir::new().unwrap();
    // A live run so the findings mirror has a home and `embed_pending`
    // can resolve the run to fill the first finding's vector — the
    // near-duplicate probe is a vector search.
    build_code_corpus(dir.path(), None).await;
    let mut store = FindingsStore::open_default(dir.path()).await.unwrap();
    let (first, _) = store
        .store_finding(
            "the connection pool reuses idle sockets instead of opening new ones".to_owned(),
            vec![],
            vec![],
            None,
        )
        .await
        .unwrap();
    store.flush().await.unwrap();
    kenn_store::embed_pending(
        &Layout::default_for(dir.path()),
        false,
        0,
        &kenn_config::EmbeddingsConfig::default().model,
        kenn_store::shared_embedder(),
    )
    .await
    .expect("embed first finding vector");

    // A near-paraphrase of the committed finding. Pre-embed so the
    // store's near-duplicate probe has a vector to query with.
    let second_text =
        "idle sockets are reused by the connection pool rather than reopened".to_owned();
    let second_vec = kenn_store::shared_embedder()
        .embed_block_until_ready(&[second_text.as_str()])
        .await
        .expect("embed near-dup query")
        .and_then(|mut v| v.drain(..).next());
    let (second, similar) = store
        .store_finding(second_text, vec![], vec![], second_vec.as_deref())
        .await
        .unwrap();
    assert_ne!(first, second, "a new finding gets its own id");
    assert!(
        similar.iter().any(|f| f.id == first),
        "the near-duplicate prior finding is surfaced; got {:?}",
        similar.iter().map(|f| f.id.as_str()).collect::<Vec<_>>()
    );
}
