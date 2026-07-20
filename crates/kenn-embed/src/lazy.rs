//! [`LazyEmbedder`] — a producer wrapper that loads its model on first
//! use and releases it after an idle TTL (design D7). Used both directly
//! by the remote `Backend::Remote` branch and as a building block.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::producer::{EmbedError, EmbedKind, EmbeddingProducer, Loader};

/// Resident model plus the bookkeeping the idle-unload task reads.
struct LazyState {
    /// The loaded producer, or `None` while unloaded.
    model: Option<Arc<dyn EmbeddingProducer>>,
    /// When the last embed call started — drives the idle TTL.
    last_used: Instant,
    /// Bumped on every load and every unload. An idle-unload task
    /// captures the generation it was spawned for and exits without
    /// acting once the generation has moved on.
    generation: u64,
}

/// A producer wrapper that loads its model on first use and releases it
/// after [`crate::IDLE_TTL`] of inactivity (design D7). Cheap to construct —
/// construction loads nothing.
pub struct LazyEmbedder {
    loader: Loader,
    idle_ttl: Duration,
    state: Arc<Mutex<LazyState>>,
}

impl LazyEmbedder {
    /// Build a lazy embedder over `loader`, releasing the model after
    /// `idle_ttl` of inactivity.
    #[must_use]
    pub fn new(loader: Loader, idle_ttl: Duration) -> Self {
        Self {
            loader,
            idle_ttl,
            state: Arc::new(Mutex::new(LazyState {
                model: None,
                last_used: Instant::now(),
                generation: 0,
            })),
        }
    }

    /// Embed a batch of text, loading the model on demand.
    ///
    /// Returns `Ok(None)` when the model cannot be loaded — no weights,
    /// no network — so callers degrade to lexical-only search rather
    /// than failing (the offline guarantee).
    pub async fn embed(
        &self,
        texts: &[&str],
        kind: EmbedKind,
    ) -> Result<Option<Vec<Vec<f32>>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let model = {
            let mut st = self.state.lock().await;
            if st.model.is_none() {
                let loader = Arc::clone(&self.loader);
                match tokio::task::spawn_blocking(move || loader()).await {
                    Ok(Ok(m)) => {
                        st.model = Some(m);
                        st.generation += 1;
                        spawn_idle_unload(Arc::clone(&self.state), self.idle_ttl, st.generation);
                    }
                    // Load failed or the blocking task panicked — degrade.
                    Ok(Err(_)) | Err(_) => return Ok(None),
                }
            }
            st.last_used = Instant::now();
            match &st.model {
                Some(m) => Arc::clone(m),
                None => return Ok(None),
            }
        };
        // The producer's `embed` is async — `RemoteEmbedder` does HTTP I/O
        // (no thread hijacking), `LlamaEmbedder` runs sync inference on
        // the calling task's thread (only used via `Backend::Local`'s
        // scheduler today, not through `LazyEmbedder`). No outer
        // `spawn_blocking` here.
        let vectors = model.embed(texts, kind).await?;
        Ok(Some(vectors))
    }

    /// Embed a single string — the query-side convenience over
    /// [`Self::embed`]. `Ok(None)` when the model is unavailable.
    pub async fn embed_query(&self, text: &str) -> Result<Option<Vec<f32>>, EmbedError> {
        Ok(self
            .embed(&[text], EmbedKind::Query)
            .await?
            .and_then(|mut v| v.drain(..).next()))
    }

    /// Token counts for `usage.prompt_tokens` accounting from the
    /// currently resident producer. Returns `None` if no model is
    /// loaded (the caller should fall back to a rough estimate, or
    /// embed first). Does NOT trigger a load — counting against an
    /// absent model is meaningless.
    pub async fn count_tokens(&self, texts: &[&str]) -> Option<Vec<usize>> {
        let st = self.state.lock().await;
        st.model.as_ref().map(|m| m.count_tokens(texts))
    }

    /// Whether a model is currently resident. Test / diagnostic only.
    #[doc(hidden)]
    pub async fn is_resident(&self) -> bool {
        self.state.lock().await.model.is_some()
    }

    /// Release the resident model, if any. Used at process exit (the
    /// bundled llama.cpp asserts at Metal-device teardown when GPU
    /// resources outlive the device, and a Rust `static` is never
    /// dropped on its own).
    pub fn release_blocking(&self) {
        if let Ok(mut state) = self.state.try_lock() {
            state.model = None;
            state.generation += 1;
        }
    }
}

/// Spawn the idle-unload task for one resident model. It sleeps a TTL,
/// then releases the model if it is still idle and still the same
/// generation; otherwise it re-sleeps (recently used) or exits
/// (superseded). A no-op when there is no Tokio runtime to spawn onto —
/// the model then stays loaded until process exit, an acceptable
/// degradation.
fn spawn_idle_unload(state: Arc<Mutex<LazyState>>, ttl: Duration, generation: u64) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(ttl).await;
            let mut st = state.lock().await;
            if st.generation != generation {
                return; // a newer load/unload owns the model now
            }
            if st.last_used.elapsed() >= ttl {
                st.model = None;
                st.generation += 1;
                return;
            }
            // Used within the window — sleep another round.
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A deterministic in-memory producer — no model, no network.
    struct FakeProducer {
        dim: usize,
    }

    #[async_trait::async_trait]
    impl EmbeddingProducer for FakeProducer {
        async fn embed(
            &self,
            texts: &[&str],
            _kind: EmbedKind,
        ) -> Result<Vec<Vec<f32>>, EmbedError> {
            Ok(texts.iter().map(|_| vec![1.0_f32; self.dim]).collect())
        }
        fn dim(&self) -> usize {
            self.dim
        }
        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "trait signature is `-> &str`; impl returns a literal — clippy suggests `&'static str` but the trait dictates the signature"
        )]
        fn model_id(&self) -> &str {
            "fake"
        }
    }

    fn failing_loader() -> Loader {
        Arc::new(|| Err(EmbedError::Backend("no model".into())))
    }

    fn counting_loader(calls: Arc<AtomicUsize>) -> Loader {
        Arc::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(FakeProducer { dim: 4 }) as Arc<dyn EmbeddingProducer>)
        })
    }

    #[tokio::test]
    async fn missing_model_degrades_to_none() {
        let lazy = LazyEmbedder::new(failing_loader(), Duration::from_secs(60));
        assert!(lazy
            .embed(&["query"], EmbedKind::Document)
            .await
            .expect("no error")
            .is_none());
        assert!(!lazy.is_resident().await);
    }

    #[tokio::test]
    async fn loads_once_then_reuses() {
        let calls = Arc::new(AtomicUsize::new(0));
        let lazy = LazyEmbedder::new(counting_loader(Arc::clone(&calls)), Duration::from_secs(60));
        for _ in 0..3 {
            assert!(lazy
                .embed(&["a", "bb"], EmbedKind::Document)
                .await
                .expect("ok")
                .is_some());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "model loaded exactly once");
        assert!(lazy.is_resident().await);
    }

    #[tokio::test]
    async fn released_after_idle_ttl() {
        let calls = Arc::new(AtomicUsize::new(0));
        let lazy = LazyEmbedder::new(
            counting_loader(Arc::clone(&calls)),
            Duration::from_millis(40),
        );
        assert!(lazy.embed_query("hello").await.expect("ok").is_some());
        assert!(lazy.is_resident().await);
        tokio::time::sleep(Duration::from_millis(160)).await;
        assert!(!lazy.is_resident().await, "model released after idle TTL");
        assert!(lazy.embed_query("again").await.expect("ok").is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 2, "reloaded after release");
    }
}
