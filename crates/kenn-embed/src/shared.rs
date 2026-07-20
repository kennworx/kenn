//! [`SharedEmbedder`] — the process-global query / corpus embedder.
//! One instance per process; selection is kicked off lazily in a
//! background tokio task by [`crate::selector::select_backend`]. Storage
//! and search callers funnel through `embed` / `embed_query` so at most
//! one producer is ever active and an in-process bulk pass cannot starve
//! an interactive query (see `embed-query-priority`).
//!
//! The host opts into embedding by calling [`init_shared_embedder`] at
//! startup with its loaded `GlobalConfig`. Hosts that never call it (and
//! tests that don't embed) get a singleton that's permanently `Disabled`
//! — embed calls return `Ok(None)` and callers degrade to lexical-only.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::Mutex;

use crate::producer::{EmbedError, EmbedKind};
use crate::scheduler;
use crate::selector::{select_backend, Backend};

/// Backing store for [`shared_embedder`].
static SHARED: OnceLock<SharedEmbedder> = OnceLock::new();

/// Cached backend-selection state. `Arc<Backend>` so callers drop the
/// lock before each embed and don't block one another.
enum Selection {
    /// Selection hasn't been kicked off yet. The next [`SharedEmbedder::backend`]
    /// call transitions to `Selecting` and spawns the background task.
    Unselected,
    /// Background selection task is running. Hot-path embed callers see
    /// [`EmbedError::Starting`] until it completes.
    Selecting,
    /// Selection ran and chose a backend.
    Active(Arc<Backend>),
    /// Embedding is opted out (no host initialized the singleton) or
    /// selection ran and yielded nothing. Kept so we don't rerun
    /// selection on every call.
    Disabled,
}

/// The public, diagnostics-facing shape of [`Selection`] — what backend the
/// process-global embedder resolved to. Returned by
/// [`SharedEmbedder::backend_kind`] for `kenn doctor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// In-process llama.cpp (`EmbeddingGemma`).
    InProcess,
    /// A remote OpenAI-compatible HTTP endpoint (kenn's own daemon or external).
    Remote,
    /// No embedder — search is lexical-only.
    Disabled,
    /// Backend selection is still running.
    Selecting,
}

impl BackendKind {
    /// The kind of a resolved [`Backend`].
    fn of(backend: &Backend) -> Self {
        match backend {
            Backend::Local(_) => BackendKind::InProcess,
            Backend::Remote(_) => BackendKind::Remote,
        }
    }
}

impl std::fmt::Display for BackendKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            BackendKind::InProcess => "in-process (llama.cpp)",
            BackendKind::Remote => "remote (HTTP)",
            BackendKind::Disabled => "disabled",
            BackendKind::Selecting => "selecting",
        };
        f.write_str(s)
    }
}

/// The process-global query / corpus embedder — one instance per process,
/// chosen lazily by [`select_backend`] on first use. Storage and search
/// callers funnel through `embed` / `embed_query` so at most one producer
/// is ever active and an in-process bulk pass cannot starve an interactive
/// query (see `embed-query-priority`).
///
/// **Non-blocking**: `backend()` never waits on selection. On the first
/// call (and after `invalidate_remote`) the selector runs in a background
/// tokio task; callers see [`EmbedError::Starting`] until it lands. This
/// preserves the "MCP responds immediately to any query" discipline.
pub struct SharedEmbedder {
    state: Arc<SharedEmbedderState>,
}

/// Mutex-shared state — held as `Arc` so the background selection task
/// can update the state independently of which `SharedEmbedder` reference
/// kicked it off (in practice there's only one, the static singleton).
struct SharedEmbedderState {
    backend: Mutex<Selection>,
    /// The host-supplied config. Threaded into `select_backend` so neither
    /// this module nor `selector` re-loads it from disk.
    cfg: Arc<kenn_config::GlobalConfig>,
}

impl SharedEmbedder {
    /// Construct a [`SharedEmbedder`] that, on first call, runs background
    /// selection against `cfg`.
    fn new(cfg: kenn_config::GlobalConfig) -> Self {
        Self {
            state: Arc::new(SharedEmbedderState {
                backend: Mutex::new(Selection::Unselected),
                cfg: Arc::new(cfg),
            }),
        }
    }

    /// Construct a permanently-disabled [`SharedEmbedder`] — what
    /// [`shared_embedder`] returns when no host has called
    /// [`init_shared_embedder`]. Every embed call yields `Ok(None)`.
    fn disabled() -> Self {
        Self {
            state: Arc::new(SharedEmbedderState {
                backend: Mutex::new(Selection::Disabled),
                cfg: Arc::new(kenn_config::GlobalConfig::default()),
            }),
        }
    }

    /// Resolve the backend. **Never blocks.** Returns `Ok(Some(b))` when
    /// selection has landed on an active backend, `Ok(None)` when
    /// selection ran but yielded nothing (embedding disabled — caller
    /// degrades to lexical-only), or `Err(Starting)` when selection is in
    /// flight (the background tokio task is running). On `Unselected`,
    /// kicks off the background task and returns `Starting` immediately.
    async fn backend(&self) -> Result<Option<Arc<Backend>>, EmbedError> {
        let mut guard = self.state.backend.lock().await;
        match &*guard {
            Selection::Active(b) => Ok(Some(Arc::clone(b))),
            Selection::Disabled => Ok(None),
            Selection::Selecting => {
                Err(EmbedError::Starting("backend selection in progress".into()))
            }
            Selection::Unselected => {
                *guard = Selection::Selecting;
                Self::spawn_selection(Arc::clone(&self.state));
                Err(EmbedError::Starting(
                    "backend selection just started".into(),
                ))
            }
        }
    }

    /// Run the selector as a regular tokio task and write the result back
    /// into shared state. `select_backend` is async — its work is HTTP
    /// probes and an optional fork-exec, so no `spawn_blocking` is needed.
    fn spawn_selection(state: Arc<SharedEmbedderState>) {
        let cfg = Arc::clone(&state.cfg);
        tokio::spawn(async move {
            let backend = select_backend(&cfg.embeddings, &cfg.server.addr).await;
            let mut guard = state.backend.lock().await;
            tracing::info!(target: "kenn_embed", "backend selection complete");
            *guard = Selection::Active(Arc::new(backend));
        });
    }

    /// Drop the cached selection iff it's a `Remote`, and kick off a
    /// background reselection. The next [`backend`](Self::backend) call
    /// sees `Selecting` and returns `Starting`. `Local` and `Disabled`
    /// are never invalidated (in-process can't be "unreachable";
    /// reselecting from `Disabled` would just refail).
    async fn invalidate_remote(&self) {
        let mut guard = self.state.backend.lock().await;
        if matches!(&*guard, Selection::Active(b) if matches!(b.as_ref(), Backend::Remote(_))) {
            tracing::info!(
                target: "kenn_embed",
                "remote embedding backend unreachable; reselecting in background"
            );
            *guard = Selection::Selecting;
            drop(guard);
            Self::spawn_selection(Arc::clone(&self.state));
        }
    }

    /// Embed against the resolved backend at the given priority.
    /// Propagates [`EmbedError::Starting`] untouched (caller retries) and
    /// translates `Unreachable` into "invalidate + Starting" so the
    /// caller's next retry sees the in-flight reselection rather than
    /// the dead remote. `Ok(None)` means embedding is disabled (caller
    /// degrades to lexical-only).
    async fn embed_with_priority(
        &self,
        texts: &[&str],
        pri: scheduler::Priority,
        kind: EmbedKind,
    ) -> Result<Option<Vec<Vec<f32>>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let Some(backend) = self.backend().await? else {
            return Ok(None);
        };
        let result = match backend.as_ref() {
            Backend::Remote(lazy) => lazy.embed(texts, kind).await.map(Option::unwrap_or_default),
            Backend::Local(sched) => {
                let owned: Vec<String> = texts.iter().map(|t| (*t).to_owned()).collect();
                sched.submit(owned, pri, kind).await
            }
        };
        match result {
            Ok(v) => Ok(Some(v)),
            Err(EmbedError::Unreachable(msg)) => {
                tracing::warn!(target: "kenn_embed", error = %msg, "remote embed unreachable; reselecting backend");
                self.invalidate_remote().await;
                Err(EmbedError::Starting(format!(
                    "remote unreachable, reselecting: {msg}"
                )))
            }
            Err(e) => Err(e),
        }
    }

    /// Bulk variant that retries internally on [`EmbedError::Starting`]
    /// with a short backoff. Use this for background jobs (corpus
    /// embed, findings ingest) where blocking briefly until the backend
    /// resolves is preferable to failing the whole batch. Hot/query
    /// paths use [`embed`](Self::embed) / [`embed_query`](Self::embed_query)
    /// directly so they can surface `Starting` to the MCP boundary
    /// immediately.
    pub async fn embed_block_until_ready(
        &self,
        texts: &[&str],
    ) -> Result<Option<Vec<Vec<f32>>>, EmbedError> {
        loop {
            match self.embed(texts).await {
                Err(EmbedError::Starting(_)) => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                other => return other,
            }
        }
    }

    /// Embed a corpus batch (document kind — raw text; low priority for
    /// the scheduler when running in-process). Returns `Ok(None)` when no
    /// embedder is available — callers degrade to lexical-only search.
    pub async fn embed(&self, texts: &[&str]) -> Result<Option<Vec<Vec<f32>>>, EmbedError> {
        self.embed_with_priority(texts, scheduler::Priority::Low, EmbedKind::Document)
            .await
    }

    /// Embed a single query string (query kind — EmbeddingGemma-family
    /// models get their query task prompt; high priority for the scheduler
    /// when running in-process). `Ok(None)` when the embedder is
    /// unavailable.
    pub async fn embed_query(&self, text: &str) -> Result<Option<Vec<f32>>, EmbedError> {
        Ok(self
            .embed_with_priority(&[text], scheduler::Priority::High, EmbedKind::Query)
            .await?
            .and_then(|mut v| v.drain(..).next()))
    }

    /// The currently-selected backend kind, for diagnostics (`kenn doctor`).
    /// Reflects the actual runtime selection — including a fallback from a
    /// dead remote to in-process — not just what the config requested. Never
    /// blocks on selection: reports [`BackendKind::Selecting`] while it runs.
    pub async fn backend_kind(&self) -> BackendKind {
        match &*self.state.backend.lock().await {
            Selection::Active(b) => BackendKind::of(b.as_ref()),
            Selection::Disabled => BackendKind::Disabled,
            Selection::Selecting | Selection::Unselected => BackendKind::Selecting,
        }
    }

    /// Release the resident model, if any. Best-effort, called at process
    /// exit (see [`release_shared_embedder`]). Uses `try_lock` to avoid
    /// blocking shutdown behind an in-flight selection.
    pub fn release_blocking(&self) {
        let Ok(guard) = self.state.backend.try_lock() else {
            return;
        };
        let Selection::Active(backend) = &*guard else {
            return;
        };
        match backend.as_ref() {
            Backend::Remote(lazy) => lazy.release_blocking(),
            Backend::Local(sched) => sched.release_blocking(),
        }
    }
}

/// Initialize the process-global embedder with the host's loaded config.
/// Call once at startup, before any embed call. First-writer-wins —
/// subsequent calls are ignored (the singleton is fixed for the process).
/// Hosts that don't want embedding skip this entirely; the lazy default
/// from [`shared_embedder`] is permanently `Disabled`.
pub fn init_shared_embedder(cfg: kenn_config::GlobalConfig) {
    // `OnceLock::set` returns the rejected value on a second-write — drop
    // it; first-writer-wins is by design (idempotent startup).
    drop(SHARED.set(SharedEmbedder::new(cfg)));
}

/// The process-global query / corpus embedder. If no host called
/// [`init_shared_embedder`], returns a permanently-disabled singleton
/// (embed yields `Ok(None)`, callers degrade to lexical-only).
pub fn shared_embedder() -> &'static SharedEmbedder {
    SHARED.get_or_init(SharedEmbedder::disabled)
}

/// Release the process-global embedder's loaded model, if any. Call
/// once just before process exit.
pub fn release_shared_embedder() {
    if let Some(shared) = SHARED.get() {
        shared.release_blocking();
    }
}
