//! Backend selection: examine the host-supplied config, probe / spawn the
//! daemon, and pick one of the four branches (external URL → running
//! daemon → spawned daemon → in-process scheduler). Async — the hot path
//! is the `/healthz` probe (one HTTP GET) and an optional fork-exec for
//! the spawn case; no CPU work happens here. Runs as a regular tokio task
//! from [`crate::SharedEmbedder::spawn_selection`]. The config is threaded
//! in from `SharedEmbedder::new` — this module never touches
//! `GlobalConfig::load`.

use std::sync::Arc;
use std::time::Duration;

use crate::lazy::LazyEmbedder;
use crate::producer::{EmbeddingProducer, Loader, DEFAULT_EMBED_DIM, IDLE_TTL};
use crate::remote::RemoteEmbedder;
use crate::{llama, scheduler, spawn};

/// Which backend the process-global embedder is running through.
pub(crate) enum Backend {
    /// Daemon or external URL — uses the existing [`LazyEmbedder`] over a
    /// remote producer; no local scheduling (the daemon schedules server-side).
    Remote(LazyEmbedder),
    /// In-process model — owns the priority scheduler (`embed_query` → high,
    /// `embed` → low), so the bulk pass never starves an interactive query.
    Local(scheduler::PriorityEmbedScheduler),
}

/// One-shot selector: probe / spawn the daemon and pick a backend from
/// the host-supplied embeddings config and the kenn-server bind address
/// (the latter lives in `[server]` because the daemon hosts more than
/// just embeddings, but the selector needs it to probe / spawn).
/// Always returns a backend — Branch 4 (in-process scheduler) is the
/// universal fallback, and its model load itself runs inside the
/// scheduler so we never report "no embedder" here.
pub(crate) async fn select_backend(
    cfg: &kenn_config::EmbeddingsConfig,
    server_addr: &str,
) -> Backend {
    // Branch 0: forced in-process. On macOS the auto-spawned daemon hits a
    // fork+Metal bug and returns empty embeddings, so the test suite sets
    // `KENN_EMBED_IN_PROCESS` to skip the probe/spawn path and run the
    // working in-process LlamaEmbedder deterministically (no shared daemon
    // to race across test binaries). Honored only in debug builds so a
    // release deployment that happens to inherit the env var can't silently
    // bypass the daemon — production that wants in-process gets it via the
    // branch-4 fallback when no daemon is reachable.
    if cfg!(debug_assertions) && std::env::var_os("KENN_EMBED_IN_PROCESS").is_some() {
        tracing::info!(
            target: "kenn_embed::selector",
            model = %cfg.model,
            "KENN_EMBED_IN_PROCESS set; using in-process LlamaEmbedder (no daemon)"
        );
        return local_backend(cfg.model.clone());
    }

    // Branch 1: explicit external URL → never spawn.
    if let Some(url) = cfg.url.as_ref() {
        tracing::info!(
            target: "kenn_embed::selector",
            url,
            model = %cfg.model,
            "using external embedding provider (no auto-spawn)"
        );
        return remote_backend(url.clone(), cfg.model.clone(), cfg.batch_size);
    }

    // Branch 2: probe the configured local addr.
    if spawn::probe_healthz(&format!("http://{server_addr}/healthz")).await {
        tracing::info!(
            target: "kenn_embed::selector",
            addr = server_addr,
            "using running local kenn server"
        );
        return remote_backend(
            format!("http://{server_addr}"),
            cfg.model.clone(),
            cfg.batch_size,
        );
    }

    // Branch 3: try to spawn one.
    match spawn::try_spawn_local_server(server_addr, Duration::from_secs(600)).await {
        Ok(()) => {
            tracing::info!(
                target: "kenn_embed::selector",
                addr = server_addr,
                "auto-spawned local kenn server"
            );
            remote_backend(
                format!("http://{server_addr}"),
                cfg.model.clone(),
                cfg.batch_size,
            )
        }
        Err(e) => {
            // Branch 4 (fallback): in-process model via the priority
            // scheduler. The in-process bulk no longer starves an interactive
            // query (the original 10-min hang).
            tracing::info!(
                target: "kenn_embed::selector",
                error = %e,
                "auto-spawn failed; using in-process LlamaEmbedder via priority scheduler"
            );
            local_backend(cfg.model.clone())
        }
    }
}

fn remote_backend(url: String, model: String, batch_size: usize) -> Backend {
    let loader: Loader = Arc::new(move || {
        Ok(Arc::new(RemoteEmbedder::new(
            &url,
            &model,
            DEFAULT_EMBED_DIM,
            batch_size,
        )) as Arc<dyn EmbeddingProducer>)
    });
    Backend::Remote(LazyEmbedder::new(loader, IDLE_TTL))
}

fn local_backend(model_id: String) -> Backend {
    let encoder_loader: scheduler::EncoderLoader = Arc::new(move || {
        llama::LlamaBatchEncoder::load(model_id.clone())
            .ok()
            .map(|e| Box::new(e) as Box<dyn scheduler::BatchEncoder>)
    });
    Backend::Local(scheduler::PriorityEmbedScheduler::new(
        encoder_loader,
        IDLE_TTL,
    ))
}
