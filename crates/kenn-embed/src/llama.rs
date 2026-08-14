//! [`LlamaEmbedder`] — the concrete in-process embedding producer:
//! `EmbeddingGemma-300M` (`q8_0` GGUF) run via `llama-cpp-2` (design D2).
//!
//! The model weights are resolved from a local cache; on a cache miss
//! they are downloaded once from the official `ggml-org` Hugging Face
//! repo. A resolve failure (no weights, no network) surfaces as an
//! [`EmbedError`], which the [`LazyEmbedder`](super::LazyEmbedder) turns
//! into the lexical-only degraded path.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use llama_cpp_2::context::params::{LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::token::LlamaToken;

use crate::producer::apply_task_prompt;

use super::{EmbedError, EmbedKind, EmbeddingProducer};

/// Official `ggml-org` `EmbeddingGemma-300M` `q8_0` GGUF — the
/// conversion that keeps the dense projection head (`benchmark.md`
/// finding 4).
const MODEL_URL: &str = "https://huggingface.co/ggml-org/embeddinggemma-300M-GGUF/resolve/main/embeddinggemma-300M-Q8_0.gguf";
/// Cached weights filename.
const MODEL_FILE: &str = "embeddinggemma-300M-Q8_0.gguf";
/// `EmbeddingGemma`'s context window — the token budget for a single
/// decode and a single sequence.
const CTX_TOKENS: usize = 2048;
/// Sequences packed into one `encode` call.
pub const SEQS_PER_BATCH: usize = 16;

/// The process-global llama.cpp backend. `LlamaBackend::init` may be
/// called only once per process; this gates that.
fn llama_backend() -> Result<&'static LlamaBackend, EmbedError> {
    static BACKEND: OnceLock<LlamaBackend> = OnceLock::new();
    static GATE: Mutex<()> = Mutex::new(());

    if let Some(b) = BACKEND.get() {
        return Ok(b);
    }
    let _gate = GATE
        .lock()
        .map_err(|e| EmbedError::Backend(format!("llama backend init lock: {e}")))?;
    // Double-checked: another thread may have initialized between the fast
    // path above and acquiring the gate. Without this re-check, `init` would
    // run twice, which llama.cpp forbids.
    if let Some(b) = BACKEND.get() {
        return Ok(b);
    }
    let backend = LlamaBackend::init()
        .map_err(|e| EmbedError::Backend(format!("llama backend init: {e}")))?;
    // `get_or_init` is infallible and returns the reference directly, so the
    // "double-init" and "missing after init" error arms this used to carry are
    // gone — both were unreachable anyway, since the gate is held and the slot
    // was just observed empty. An `init` failure still returns above without
    // touching the slot, so a later call retries rather than caching the error.
    Ok(BACKEND.get_or_init(|| backend))
}

/// GPU layer count: offload everything on macOS (Metal), CPU elsewhere.
const fn n_gpu_layers() -> u32 {
    #[cfg(target_os = "macos")]
    {
        999
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

/// EmbeddingGemma-300M behind `llama-cpp-2`. `LlamaModel` is `Send +
/// Sync`, so this is too — the weights are immutable after load and a
/// fresh context is created per [`embed`](EmbeddingProducer::embed) call.
///
/// Touching this type? Run `just embed-smoke` — it loads the real
/// GGUF and exercises [`load`](Self::load) + [`embed`](EmbeddingProducer::embed)
/// end-to-end, catching tokenizer / pooling / `llama-cpp-2`-upgrade
/// regressions that no unit test sees.
pub struct LlamaEmbedder {
    model: LlamaModel,
    dim: usize,
    model_id: String,
}

impl LlamaEmbedder {
    /// Resolve the model weights (downloading on a cache miss) and load
    /// them. Blocking — the caller runs it off the async path. The
    /// `model_id` is stored verbatim and returned by [`identity`](Self::identity)
    /// — it's the public name (e.g. `embeddinggemma-300M`) that
    /// downstream sidecar-manifest writes stamp.
    ///
    /// The opt-in policy lives in
    /// [`shared_embedder`](super::shared_embedder); production callers go
    /// through `init_shared_embedder` so this never runs without a host
    /// having asked for embedding.
    pub fn load(model_id: String) -> Result<Self, EmbedError> {
        let path = resolve_model_path()?;
        let backend = llama_backend()?;
        let params = LlamaModelParams::default().with_n_gpu_layers(n_gpu_layers());
        let model = LlamaModel::load_from_file(backend, &path, &params).map_err(|e| {
            EmbedError::Backend(format!("load embedding model {}: {e}", path.display()))
        })?;
        let dim = usize::try_from(model.n_embd()).map_err(|e| {
            EmbedError::Backend(format!("embedding model reports a bad dimension: {e}"))
        })?;
        Ok(Self {
            model,
            dim,
            model_id,
        })
    }
}

impl LlamaEmbedder {
    /// CPU-bound synchronous embed — used by [`LlamaBatchEncoder`] inside
    /// the scheduler worker (a thread or a `spawn_blocking` task), and
    /// internally by the async [`EmbeddingProducer::embed`] impl below.
    /// Pulled out so both call sites share the inference body without a
    /// per-call tokio runtime to bridge sync ↔ async.
    pub(crate) fn embed_sync(
        &self,
        texts: &[&str],
        kind: EmbedKind,
    ) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        // Query texts carry the model's task prompt; document texts stay
        // raw, so stored corpus vectors are unaffected by prompting.
        let prompted = apply_task_prompt(&self.model_id, kind, texts);
        let texts: Vec<&str> = prompted.as_ref().map_or_else(
            || texts.to_vec(),
            |p| p.iter().map(String::as_str).collect(),
        );
        let mut ctx = self.new_embedding_context()?;
        let tokenized = self.tokenize_all(&texts)?;
        let lens: Vec<usize> = tokenized.iter().map(Vec::len).collect();
        encode_batches(&mut ctx, &tokenized, &plan_batches(&lens))
    }

    /// A fresh embedding context. One per `embed_sync` call — the weights are
    /// immutable and shared, the context is not.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "CTX_TOKENS (2048) and SEQS_PER_BATCH (16) are small literal constants, well within u32"
    )]
    fn new_embedding_context(&self) -> Result<LlamaContext<'_>, EmbedError> {
        let backend = llama_backend()?;
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(CTX_TOKENS as u32))
            .with_n_batch(CTX_TOKENS as u32)
            .with_n_ubatch(CTX_TOKENS as u32)
            .with_n_seq_max(SEQS_PER_BATCH as u32)
            .with_embeddings(true)
            // Unspecified → llama.cpp uses the GGUF's own declared
            // pooling type (MEAN for EmbeddingGemma).
            .with_pooling_type(LlamaPoolingType::Unspecified);
        self.model
            .new_context(backend, params)
            .map_err(|e| EmbedError::Backend(format!("create embedding context: {e}")))
    }

    /// Tokenize every text, clamped to the context window. `EmbeddingGemma` is
    /// BERT-family, so BOS is always added.
    fn tokenize_all(&self, texts: &[&str]) -> Result<Vec<Vec<LlamaToken>>, EmbedError> {
        let mut out = Vec::with_capacity(texts.len());
        for text in texts {
            let mut toks = self
                .model
                .str_to_token(text, AddBos::Always)
                .map_err(|e| EmbedError::Backend(format!("tokenize text: {e}")))?;
            // `truncate` is already a no-op when the vector is shorter, so the
            // length guard it used to carry was dead weight.
            toks.truncate(CTX_TOKENS);
            out.push(toks);
        }
        Ok(out)
    }
}

/// Run one `encode` per planned batch and collect the pooled vectors.
#[expect(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    reason = "SEQS_PER_BATCH is the literal constant 16, well within i32"
)]
fn encode_batches(
    ctx: &mut LlamaContext<'_>,
    tokenized: &[Vec<LlamaToken>],
    plan: &[usize],
) -> Result<Vec<Vec<f32>>, EmbedError> {
    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(tokenized.len());
    let mut batch = LlamaBatch::new(CTX_TOKENS, SEQS_PER_BATCH as i32);
    let mut next = 0usize;
    for &seqs in plan {
        batch.clear();
        let filled = fill_batch(&mut batch, tokenized, next, seqs)?;
        // Read back what was actually added, not what the plan asked for. The
        // two agree for any plan `plan_batches` produced, but they are computed
        // in different functions, and reading more sequences than were encoded
        // would pull slots this batch never wrote.
        let added = filled - next;
        next = filled;
        // BERT-family encoder: route through `encode` (no kv-cache).
        ctx.encode(&mut batch)
            .map_err(|e| EmbedError::Backend(format!("encode embedding batch: {e}")))?;
        vectors.extend(read_batch_embeddings(ctx, added)?);
    }
    Ok(vectors)
}

/// Add `seqs` sequences starting at `next` to `batch`; returns the new cursor.
///
/// `logits_all = true`: mark every token of the sequence as an output.
/// `EmbeddingGemma` mean-pools over all tokens, so llama.cpp needs them all
/// flagged — passing `false` marks only the last token and llama.cpp then logs
/// an "overriding" notice as it corrects it.
fn fill_batch(
    batch: &mut LlamaBatch,
    tokenized: &[Vec<LlamaToken>],
    next: usize,
    seqs: usize,
) -> Result<usize, EmbedError> {
    let mut cursor = next;
    for slot in 0..seqs {
        let Some(toks) = tokenized.get(cursor) else {
            break;
        };
        batch
            .add_sequence(toks, i32::try_from(slot).unwrap_or(i32::MAX), true)
            .map_err(|e| EmbedError::Backend(format!("add sequence to batch: {e}")))?;
        cursor += 1;
    }
    Ok(cursor)
}

/// Read and L2-normalize the pooled embedding of each sequence in the batch.
fn read_batch_embeddings(ctx: &LlamaContext<'_>, seqs: usize) -> Result<Vec<Vec<f32>>, EmbedError> {
    let mut out = Vec::with_capacity(seqs);
    for seq in 0..seqs {
        let raw = ctx
            .embeddings_seq_ith(i32::try_from(seq).unwrap_or(i32::MAX))
            .map_err(|e| EmbedError::Backend(format!("read embedding: {e}")))?;
        let mut v = raw.to_vec();
        l2_normalize(&mut v);
        out.push(v);
    }
    Ok(out)
}

/// How many sequences go into each `encode` call, from each text's token count.
///
/// Pure arithmetic over lengths, so the packing rule is testable without a
/// model even though everything around it needs one — and packing is where the
/// interesting decision lives.
///
/// A batch takes up to [`SEQS_PER_BATCH`] sequences and stays within the
/// [`CTX_TOKENS`] budget, **except** that it always takes at least one: a
/// single text at or over the budget must still ride alone rather than wedge
/// the loop. That `seqs > 0` guard is why the outer loop always advances, so
/// no batch is ever planned empty.
fn plan_batches(lens: &[usize]) -> Vec<usize> {
    let mut plan = Vec::new();
    let mut i = 0usize;
    while i < lens.len() {
        let mut seqs = 0usize;
        let mut tokens = 0usize;
        while seqs < SEQS_PER_BATCH {
            let Some(&len) = lens.get(i) else {
                break;
            };
            if seqs > 0 && tokens + len > CTX_TOKENS {
                break; // flush; this text rides the next encode
            }
            tokens += len;
            seqs += 1;
            i += 1;
        }
        plan.push(seqs);
    }
    plan
}

#[async_trait::async_trait]
impl EmbeddingProducer for LlamaEmbedder {
    /// The async producer surface delegates to the sync inference body.
    /// `LazyEmbedder` (the only user of this through the trait object) is
    /// today only used with `RemoteEmbedder`; if a future caller threads
    /// `LlamaEmbedder` through here, they should wrap this call in
    /// `spawn_blocking` themselves.
    async fn embed(&self, texts: &[&str], kind: EmbedKind) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.embed_sync(texts, kind)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        // The id is fixed at construction time — the caller passes the
        // value resolved from its config load. Manifest writers receive
        // the same value via parameter so the stamp matches model_id().
        &self.model_id
    }

    fn count_tokens(&self, texts: &[&str]) -> Vec<usize> {
        // Use the same `AddBos::Always` policy as `embed`, so the
        // count reported in `usage.prompt_tokens` matches what
        // inference actually saw.
        texts
            .iter()
            .map(|t| {
                self.model
                    .str_to_token(t, AddBos::Always)
                    .map_or(1, |v| v.len())
            })
            .collect()
    }
}

/// A [`scheduler::BatchEncoder`](crate::scheduler::BatchEncoder) backed by the
/// in-process [`LlamaEmbedder`]. Lives on the scheduler's dedicated thread; it
/// **reuses the loaded model** across batches. A fresh `LlamaContext` is built
/// per `encode_batch` call (a struct holding both the model and a context that
/// borrows it is self-referential and not expressible in safe Rust without
/// `ouroboros`); the context is lightweight relative to the encode, and a query
/// is a single batch, so the per-batch context only adds minor overhead to the
/// background bulk pass.
pub struct LlamaBatchEncoder {
    inner: LlamaEmbedder,
}

impl LlamaBatchEncoder {
    /// Load the model (blocking) and wrap it as a batch encoder. `model_id`
    /// is the public id (e.g. `embeddinggemma-300M`) stamped into the
    /// sidecar manifest and returned by `identity()`.
    pub fn load(model_id: String) -> Result<Self, EmbedError> {
        Ok(Self {
            inner: LlamaEmbedder::load(model_id)?,
        })
    }
}

#[async_trait::async_trait(?Send)]
impl crate::scheduler::BatchEncoder for LlamaBatchEncoder {
    async fn encode_batch(
        &mut self,
        texts: &[String],
        kind: EmbedKind,
    ) -> Result<Vec<Vec<f32>>, EmbedError> {
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        // CPU-bound inference runs inline on the scheduler thread's
        // current-thread runtime — blocking that runtime is fine because
        // the thread is dedicated to encoding (priority-queue checks
        // resume between batches).
        self.inner.embed_sync(&refs, kind)
    }

    fn batch_size(&self) -> usize {
        SEQS_PER_BATCH
    }
}

/// L2-normalize a vector in place — cosine similarity then reduces to a
/// dot product, matching the `Cosine` metric of the Lance vector index.
fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Resolve the GGUF weights path: an explicit `KENN_EMBED_MODEL_PATH`
/// override, else the local cache, else a one-time download from the
/// `ggml-org` repo into the cache.
///
/// (`KENN_EMBED_MODEL` is reserved for the model *id string* sent in
/// `/v1/embeddings` and stamped in the manifest — see `kenn-config`'s
/// `GlobalConfig.embeddings.model`. The filesystem-path override gets
/// the explicit `_PATH` suffix.)
fn resolve_model_path() -> Result<PathBuf, EmbedError> {
    let explicit = std::env::var("KENN_EMBED_MODEL_PATH").ok();
    let dir = model_cache_dir()?;
    let outcome = classify_model_path(explicit.as_deref(), dir.join(MODEL_FILE), Path::is_file);
    apply_model_path_outcome(outcome, &dir)
}

/// Side-effecting tail of [`resolve_model_path`] — turns a
/// `ModelPathOutcome` into either the resolved path or the download
/// side-effect. Extracted so the dispatcher above stays at CC ≤ 3 and
/// each non-download arm is unit-testable.
fn apply_model_path_outcome(
    outcome: ModelPathOutcome,
    cache_dir: &Path,
) -> Result<PathBuf, EmbedError> {
    match outcome {
        ModelPathOutcome::UseExplicit(p) | ModelPathOutcome::UseCache(p) => Ok(p),
        ModelPathOutcome::ExplicitMissing(p) => Err(EmbedError::Backend(format!(
            "KENN_EMBED_MODEL_PATH points at a missing file: {}",
            p.display()
        ))),
        ModelPathOutcome::Download(p) => {
            std::fs::create_dir_all(cache_dir)?;
            download_model(&p)?;
            Ok(p)
        }
    }
}

/// Outcome of the model-path decision — what to do given the env var and
/// cache state. Pure values; no filesystem effects.
#[derive(Debug, PartialEq, Eq)]
enum ModelPathOutcome {
    /// Use the explicit path from `KENN_EMBED_MODEL_PATH`.
    UseExplicit(PathBuf),
    /// `KENN_EMBED_MODEL_PATH` was set but the file doesn't exist —
    /// caller emits the user-facing error.
    ExplicitMissing(PathBuf),
    /// No explicit override; the standard cache file is present.
    UseCache(PathBuf),
    /// No explicit override; the cache file is missing — caller
    /// downloads it.
    Download(PathBuf),
}

/// Pure classifier for [`resolve_model_path`]. Inputs:
/// - `explicit`: `Some(s)` when `KENN_EMBED_MODEL_PATH` is set; empty
///   string is treated as unset (matches the original behaviour).
/// - `cache_path`: the standard `<cache>/embeddinggemma-300M-Q8_0.gguf`
///   path the caller would otherwise download into.
/// - `file_exists`: dependency-injected filesystem probe. The real
///   caller passes `|p| p.is_file()`; tests pass a closure that
///   matches a known set of paths.
fn classify_model_path(
    explicit: Option<&str>,
    cache_path: PathBuf,
    file_exists: impl Fn(&Path) -> bool,
) -> ModelPathOutcome {
    if let Some(s) = explicit.filter(|s| !s.is_empty()) {
        let path = PathBuf::from(s);
        return if file_exists(&path) {
            ModelPathOutcome::UseExplicit(path)
        } else {
            ModelPathOutcome::ExplicitMissing(path)
        };
    }
    if file_exists(&cache_path) {
        ModelPathOutcome::UseCache(cache_path)
    } else {
        ModelPathOutcome::Download(cache_path)
    }
}

/// `$XDG_CACHE_HOME/kenn/models`, falling back to `$HOME/.cache/kenn/models`.
fn model_cache_dir() -> Result<PathBuf, EmbedError> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return Ok(PathBuf::from(xdg).join("kenn").join("models"));
        }
    }
    let home = std::env::var("HOME")
        .map_err(|_e| EmbedError::Backend("cannot resolve model cache: $HOME is unset".into()))?;
    Ok(PathBuf::from(home)
        .join(".cache")
        .join("kenn")
        .join("models"))
}

/// Download the GGUF to a `.part` sibling, then atomically rename — so a
/// crash mid-download never leaves a truncated file that looks complete.
fn download_model(dest: &Path) -> Result<(), EmbedError> {
    let part = dest.with_extension("part");
    let status = Command::new("curl")
        .args(["--fail", "--show-error", "--location", "--retry", "3", "-o"])
        .arg(&part)
        .arg(MODEL_URL)
        .status()
        .map_err(|e| EmbedError::Backend(format!("spawn curl to download model: {e}")))?;
    if !status.success() {
        drop(std::fs::remove_file(&part));
        return Err(EmbedError::Backend(format!(
            "model download failed ({status}); no embedding model available"
        )));
    }
    std::fs::rename(&part, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> PathBuf {
        PathBuf::from("/cache/model.gguf")
    }
    fn explicit_path() -> PathBuf {
        PathBuf::from("/explicit/model.gguf")
    }

    #[test]
    fn classify_model_path_uses_explicit_when_set_and_present() {
        let out = classify_model_path(Some("/explicit/model.gguf"), cache(), |p| {
            p == explicit_path()
        });
        assert_eq!(out, ModelPathOutcome::UseExplicit(explicit_path()));
    }

    #[test]
    fn classify_model_path_reports_explicit_missing() {
        let out = classify_model_path(Some("/explicit/model.gguf"), cache(), |_| false);
        assert_eq!(out, ModelPathOutcome::ExplicitMissing(explicit_path()));
    }

    #[test]
    fn classify_model_path_treats_empty_env_as_unset() {
        let out = classify_model_path(Some(""), cache(), |p| p == cache());
        assert_eq!(out, ModelPathOutcome::UseCache(cache()));
    }

    #[test]
    fn classify_model_path_uses_cache_when_present() {
        let out = classify_model_path(None, cache(), |_| true);
        assert_eq!(out, ModelPathOutcome::UseCache(cache()));
    }

    #[test]
    fn classify_model_path_downloads_when_cache_missing() {
        let out = classify_model_path(None, cache(), |_| false);
        assert_eq!(out, ModelPathOutcome::Download(cache()));
    }

    #[test]
    fn apply_outcome_returns_path_for_use_arms() {
        let p = PathBuf::from("/x/model.gguf");
        let dir = Path::new("/cache");
        assert_eq!(
            apply_model_path_outcome(ModelPathOutcome::UseExplicit(p.clone()), dir).unwrap(),
            p
        );
        assert_eq!(
            apply_model_path_outcome(ModelPathOutcome::UseCache(p.clone()), dir).unwrap(),
            p
        );
    }

    #[test]
    fn apply_outcome_errors_on_explicit_missing() {
        let p = PathBuf::from("/missing/model.gguf");
        let err =
            apply_model_path_outcome(ModelPathOutcome::ExplicitMissing(p), Path::new("/cache"))
                .unwrap_err();
        let EmbedError::Backend(msg) = err else {
            panic!("expected EmbedError::Backend, got {err:?}");
        };
        assert!(msg.contains("KENN_EMBED_MODEL_PATH"));
        assert!(msg.contains("missing"));
    }

    // ── batch packing ────────────────────────────────────────────────
    // Previously inline in `embed_sync`, so it could only be exercised with
    // real weights loaded — which the default suite never does. As arithmetic
    // over token counts it needs no model at all.

    #[test]
    fn short_texts_pack_into_one_batch() {
        assert_eq!(plan_batches(&[10, 20, 30]), vec![3]);
    }

    #[test]
    fn a_batch_never_exceeds_the_sequence_cap() {
        let lens = vec![1; SEQS_PER_BATCH * 2 + 3];
        let plan = plan_batches(&lens);
        assert_eq!(plan, vec![SEQS_PER_BATCH, SEQS_PER_BATCH, 3]);
        assert!(plan.iter().all(|&s| s <= SEQS_PER_BATCH));
    }

    #[test]
    fn a_batch_never_exceeds_the_token_budget() {
        // Three texts at half the window: two fit, the third flushes.
        let half = CTX_TOKENS / 2;
        assert_eq!(plan_batches(&[half, half, half]), vec![2, 1]);
    }

    #[test]
    fn an_oversized_text_rides_alone_rather_than_wedging_the_loop() {
        // The `seqs > 0` guard, exercised by a length STRICTLY over the budget
        // — at exactly `CTX_TOKENS` the comparison is `2048 + 0 > 2048`, which
        // is false, so the guard is never consulted and this would pass for the
        // wrong reason. Over the budget, without the guard, the inner loop
        // breaks immediately, `seqs` stays 0, `i` never advances, and the outer
        // loop spins forever.
        //
        // `tokenize_all` truncates to `CTX_TOKENS`, so this input cannot arise
        // today; the guard is what keeps that a bounded assumption rather than
        // a hang if truncation ever moves.
        let plan = plan_batches(&[CTX_TOKENS + 1, 5, 5]);
        assert_eq!(plan, vec![1, 2]);
        assert!(
            plan.iter().all(|&s| s > 0),
            "a planned batch is never empty: {plan:?}"
        );
    }

    #[test]
    fn every_text_is_planned_exactly_once() {
        let lens = vec![300, 900, 1500, 40, 2048, 7, 800, 800, 800];
        let planned: usize = plan_batches(&lens).iter().sum();
        assert_eq!(planned, lens.len(), "no text dropped or double-counted");
    }

    #[test]
    fn no_texts_plans_no_batches() {
        assert!(plan_batches(&[]).is_empty());
    }
}
