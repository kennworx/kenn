//! The pluggable embedding-producer boundary — the trait every backend
//! satisfies, plus the shared error type, loader signature, and the
//! handful of related constants. Lives separately from `lazy` / `shared`
//! / `selector` so the boundary types form one small, dependency-free
//! module.

use std::sync::Arc;
use std::time::Duration;

/// Errors from the embedding boundary. Storage layers convert via
/// `From<EmbedError> for <their error>`.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("backend: {0}")]
    Backend(String),
    /// Transport-level failure reaching a remote embedder (connection
    /// refused, DNS, TCP timeout). Distinct from `Backend` so the
    /// [`crate::SharedEmbedder`] can invalidate the cached `Remote`
    /// selection and reselect (e.g. the daemon idle-exited between probe
    /// and request).
    #[error("unreachable: {0}")]
    Unreachable(String),
    /// Backend selection is running in the background (cold start or
    /// reselection after `SharedEmbedder::invalidate_remote`). The
    /// caller should treat this as "not yet available, try again
    /// shortly" — it is **not** a failure. The MCP boundary surfaces it
    /// as `EMBEDDER_STARTING` so the agent retries; bulk callers wrap
    /// the call in a short retry loop (see `embed_block_until_ready`).
    #[error("starting: {0}")]
    Starting(String),
}

/// What the text being embedded *is* — a free-text search query or a
/// corpus document (code symbol, finding). Distinct from the scheduler's
/// interactive-vs-bulk `Priority`: kind selects the model's task prompt,
/// priority selects queue order, and the two must not piggyback on each
/// other (`embedding-gemma-prompts` design D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedKind {
    /// A free-text search query. EmbeddingGemma-family models get their
    /// query task prompt prepended at embed time.
    Query,
    /// Corpus text (code symbol, finding). Always embedded raw — the
    /// document prompt measured as not worth its re-embed cost and is
    /// deferred, so stored vectors are unaffected by prompting.
    Document,
}

/// `EmbeddingGemma`'s query-side task prompt, from the model card's
/// `prompts` map. Confirmed by the `prompt_ab` A/B eval (r@1 0.660 →
/// 0.695 on the isolated vector arm).
const GEMMA_QUERY_PROMPT: &str = "task: search result | query: ";

/// The task-instruction prefix `model_id` wants for `kind`, or `None`
/// for raw text. Implemented once and shared by the in-process and
/// remote producers so the two transports can never disagree
/// (`embedding-gemma-prompts` design D1). Query-only scope: `Document`
/// is always raw.
fn task_prompt(model_id: &str, kind: EmbedKind) -> Option<&'static str> {
    match kind {
        EmbedKind::Query if is_embedding_gemma(model_id) => Some(GEMMA_QUERY_PROMPT),
        EmbedKind::Query | EmbedKind::Document => None,
    }
}

/// EmbeddingGemma-family detection over the configured model id.
/// `contains` (not `starts_with`) so provider-prefixed spellings like
/// `ggml-org/embeddinggemma-300M` or `embeddinggemma:300m` match too.
fn is_embedding_gemma(model_id: &str) -> bool {
    model_id.to_ascii_lowercase().contains("embeddinggemma")
}

/// Prepend `model_id`'s task prompt for `kind` to every text. `None`
/// when no prompt applies (the common case) so callers keep the borrowed
/// originals and allocate nothing.
pub(crate) fn apply_task_prompt(
    model_id: &str,
    kind: EmbedKind,
    texts: &[&str],
) -> Option<Vec<String>> {
    let prompt = task_prompt(model_id, kind)?;
    Some(texts.iter().map(|t| format!("{prompt}{t}")).collect())
}

/// The pluggable embedding-producer boundary: a batch of text in, a
/// batch of fixed-dimension float vectors out, with the dimension
/// exposed. All embedding generation goes through this trait.
///
/// `embed` is `async fn` because the natural producer for remote backends
/// (`RemoteEmbedder`) does HTTP I/O. CPU-bound implementations
/// (`LlamaEmbedder`) wrap their inference in `spawn_blocking` internally
/// so they don't hijack runtime threads.
#[async_trait::async_trait]
pub trait EmbeddingProducer: Send + Sync {
    /// Embed a batch of text, one fixed-dimension vector per input, in
    /// input order. Every returned vector has length [`Self::dim`].
    /// `kind` says what the text is — producers whose model wants a
    /// task prompt for that kind prepend it before tokenizing.
    async fn embed(&self, texts: &[&str], kind: EmbedKind) -> Result<Vec<Vec<f32>>, EmbedError>;

    /// The fixed dimension of every vector this producer emits.
    fn dim(&self) -> usize;

    /// The producing model's id — the value stamped into the sidecar
    /// manifest's `embedding_model.id` field. Two producers configured
    /// for the same model id return the same string regardless of which
    /// transport backs them.
    fn model_id(&self) -> &str;

    /// Token counts for OpenAI-style `usage.prompt_tokens` accounting,
    /// one per input. Default: a rough char-based estimate so the
    /// trait is implementable without exposing a tokenizer. Concrete
    /// producers (e.g. [`crate::LlamaEmbedder`]) override with real counts.
    fn count_tokens(&self, texts: &[&str]) -> Vec<usize> {
        texts.iter().map(|t| (t.len() / 4).max(1)).collect()
    }
}

/// Builds a producer on demand. `Arc` (not `Box`) so the closure can be
/// cloned into a `spawn_blocking` for the load, which may download the
/// model and takes seconds.
pub type Loader = Arc<dyn Fn() -> Result<Arc<dyn EmbeddingProducer>, EmbedError> + Send + Sync>;

/// How long a loaded model stays resident with no embed calls before the
/// idle-unload task releases it (design D7).
pub const IDLE_TTL: Duration = Duration::from_secs(60);

/// Default embedding dimension reported by `RemoteEmbedder` when we
/// haven't yet round-tripped a real embed call. Matches
/// EmbeddingGemma-300M; should agree with the lance schema's
/// `EMBEDDING_DIM` constant in kenn-store.
pub const DEFAULT_EMBED_DIM: usize = 768;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemma_query_kind_gets_the_query_prompt() {
        let out = apply_task_prompt(
            "embeddinggemma-300M",
            EmbedKind::Query,
            &["find the parser"],
        );
        assert_eq!(
            out,
            Some(vec![
                "task: search result | query: find the parser".to_owned()
            ])
        );
    }

    #[test]
    fn gemma_document_kind_stays_raw() {
        // The doc prompt is deferred — Document must allocate nothing so
        // corpus embedding output is byte-identical to the unprompted era.
        assert_eq!(
            apply_task_prompt("embeddinggemma-300M", EmbedKind::Document, &["fn parse()"]),
            None
        );
    }

    #[test]
    fn non_gemma_models_get_no_prompt_for_either_kind() {
        for kind in [EmbedKind::Query, EmbedKind::Document] {
            assert_eq!(apply_task_prompt("nomic-embed-text", kind, &["q"]), None);
        }
    }

    #[test]
    fn gemma_detection_matches_prefixed_and_cased_spellings() {
        for id in [
            "embeddinggemma-300M",
            "EmbeddingGemma-300m",
            "ggml-org/embeddinggemma-300M-GGUF",
            "embeddinggemma:300m",
        ] {
            assert!(is_embedding_gemma(id), "{id} should match");
        }
        assert!(!is_embedding_gemma("gemma-2b")); // generative gemma is not the embedder
    }
}
