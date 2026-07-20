//! The `EmbeddingsModule` host module: builds the priority scheduler +
//! embedding producer and wires the router, plus the test-friendly
//! `ProducerBatchEncoder` shim.

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use kenn_embed::llama::{LlamaBatchEncoder, SEQS_PER_BATCH};
use kenn_embed::scheduler::{BatchEncoder, EncoderLoader, PriorityEmbedScheduler};
use kenn_embed::{EmbedError, EmbedKind, EmbeddingProducer, Loader, IDLE_TTL};

use crate::host::Module;

use super::{embeddings_handler, models_handler};

/// Wraps an existing [`EmbeddingProducer`] as a [`BatchEncoder`] so tests can
/// feed a fake producer into the scheduler. Production builds construct
/// [`LlamaBatchEncoder`] directly via the default loader.
struct ProducerBatchEncoder {
    producer: Arc<dyn EmbeddingProducer>,
}

#[async_trait::async_trait(?Send)]
impl BatchEncoder for ProducerBatchEncoder {
    async fn encode_batch(
        &mut self,
        texts: &[String],
        kind: EmbedKind,
    ) -> Result<Vec<Vec<f32>>, EmbedError> {
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        // The scheduler's worker loop is async on its own current-thread
        // runtime — await the producer directly. No per-call runtime
        // bridge needed.
        self.producer.embed(&refs, kind).await
    }

    fn batch_size(&self) -> usize {
        SEQS_PER_BATCH
    }
}

/// The embeddings capability module — owns the shared priority embedding
/// scheduler (`embed-query-priority`). The handler classifies each request
/// (cardinality + optional `X-Kenn-Priority`), submits, and shapes the
/// `OpenAI` response. Construct via [`Self::new`] / [`Self::with_loader`] and
/// pass to [`Host::with_module`](crate::Host::with_module).
pub struct EmbeddingsModule {
    /// Resolved model id; advertised by `/v1/models` and validated
    /// against the `model` field of `/v1/embeddings` requests.
    pub(crate) model_id: String,
    /// The shared priority scheduler — drains high (query) before low (bulk)
    /// at `SEQS_PER_BATCH` granularity. Held in an `Arc` so the handler can
    /// share a reference across requests.
    pub(crate) scheduler: Arc<PriorityEmbedScheduler>,
}

impl EmbeddingsModule {
    /// New embeddings module with `model_id` for `/v1/models`. Production
    /// builds load the in-process [`LlamaBatchEncoder`] lazily on the
    /// scheduler's worker thread.
    #[must_use]
    pub fn new(model_id: String) -> Self {
        let model_id_for_loader = model_id.clone();
        let encoder_loader: EncoderLoader = Arc::new(move || {
            LlamaBatchEncoder::load(model_id_for_loader.clone())
                .ok()
                .map(|e| Box::new(e) as Box<dyn BatchEncoder>)
        });
        Self::with_encoder_loader(model_id, encoder_loader)
    }

    /// New embeddings module with an explicit `Loader` — tests inject a
    /// deterministic in-memory [`EmbeddingProducer`] here. The producer is
    /// wrapped as a [`BatchEncoder`] via [`ProducerBatchEncoder`].
    #[must_use]
    pub fn with_loader(model_id: String, loader: Loader) -> Self {
        let encoder_loader: EncoderLoader = Arc::new(move || {
            loader().ok().map(|producer| {
                Box::new(ProducerBatchEncoder { producer }) as Box<dyn BatchEncoder>
            })
        });
        Self::with_encoder_loader(model_id, encoder_loader)
    }

    /// New embeddings module with an explicit [`EncoderLoader`] — the
    /// scheduler's native loader shape.
    #[must_use]
    fn with_encoder_loader(model_id: String, encoder_loader: EncoderLoader) -> Self {
        let scheduler = Arc::new(PriorityEmbedScheduler::new(encoder_loader, IDLE_TTL));
        Self {
            model_id,
            scheduler,
        }
    }
}

// ============== Module trait impl ==================================

impl Module for EmbeddingsModule {
    fn name(&self) -> &'static str {
        "embeddings"
    }

    fn register(self: Arc<Self>, router: Router) -> Router {
        // Build a substate-typed sub-router, then erase the state
        // via `with_state` before merging into the host's stateless
        // router. This is the axum 0.8 idiom for mounting a module
        // with its own state into a `Router<()>`.
        let sub: Router = Router::new()
            .route("/v1/embeddings", post(embeddings_handler))
            .route("/v1/models", get(models_handler))
            .with_state(self);
        router.merge(sub)
    }

    fn shutdown<'a>(
        self: Arc<Self>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // Release the scheduler's resident model (if any). Matches the
            // `release_shared_embedder()` shutdown hook in kenn-cli —
            // bundled llama.cpp asserts at Metal-device teardown when
            // GPU resources outlive the device.
            self.scheduler.release_blocking();
        })
    }
}
