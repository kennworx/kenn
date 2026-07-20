//! `kenn doctor` — actively probe the embedder and report its health.
//!
//! Unlike `kenn status` (which reads the persisted snapshot), this exercises the
//! *actually-selected* embedding backend by embedding a trivial string, so it
//! surfaces runtime failures — notably the macOS daemon fork+Metal bug that
//! otherwise degrades search to lexical-only silently.

use std::time::{Duration, Instant};

use anyhow::Result;
use kenn_embed::{BackendKind, EmbedError};

use crate::exit::ExitCodes;

/// Give background backend selection a few seconds to land before giving up.
const SELECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The three outcomes of a probe, decoupled from the embed call so the
/// classification + exit-code logic is testable without a live embedder.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Healthy { dim: usize },
    Disabled,
    Failed { error: String },
}

fn classify(result: Result<Option<Vec<f32>>, EmbedError>) -> Outcome {
    match result {
        Ok(Some(v)) => Outcome::Healthy { dim: v.len() },
        Ok(None) => Outcome::Disabled,
        Err(e) => Outcome::Failed {
            error: e.to_string(),
        },
    }
}

/// Print the outcome and return the process exit code. Healthy and disabled are
/// both success (disabled is a valid lexical-only configuration); a backend
/// failure is a non-zero exit carrying the raw error.
fn report(outcome: &Outcome, model_id: &str, backend: BackendKind, latency: Duration) -> ExitCodes {
    match outcome {
        Outcome::Healthy { dim } => {
            println!("embedder: healthy");
            println!("  model:   {model_id}");
            println!("  backend: {backend}");
            println!("  dim:     {dim}");
            println!("  latency: {} ms", latency.as_millis());
            ExitCodes::Ok
        }
        Outcome::Disabled => {
            println!("embedder: disabled — no model configured; search is lexical-only");
            println!("  backend: {backend}");
            ExitCodes::Ok
        }
        Outcome::Failed { error } => {
            eprintln!("embedder: FAILED");
            eprintln!("  model:   {model_id}");
            eprintln!("  backend: {backend}");
            eprintln!("  error:   {error}");
            ExitCodes::Generic
        }
    }
}

pub fn run(model_id: &str) -> Result<ExitCodes> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(probe(model_id))
}

async fn probe(model_id: &str) -> Result<ExitCodes> {
    let embedder = kenn_store::shared_embedder();
    let start = Instant::now();
    // Retry while background selection is still running (`Starting`).
    let result = loop {
        match embedder.embed_query("hello").await {
            Err(EmbedError::Starting(_)) if start.elapsed() < SELECT_TIMEOUT => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            other => break other,
        }
    };
    let latency = start.elapsed();
    let backend = embedder.backend_kind().await;
    Ok(report(&classify(result), model_id, backend, latency))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_maps_each_outcome() {
        assert_eq!(
            classify(Ok(Some(vec![0.0; 768]))),
            Outcome::Healthy { dim: 768 }
        );
        assert_eq!(classify(Ok(None)), Outcome::Disabled);
        assert_eq!(
            classify(Err(EmbedError::Backend("boom".into()))),
            Outcome::Failed {
                error: "backend: boom".into()
            }
        );
    }

    #[test]
    fn report_exit_codes_distinguish_outcomes() {
        let d = Duration::from_millis(1);
        assert_eq!(
            report(
                &Outcome::Healthy { dim: 768 },
                "m",
                BackendKind::InProcess,
                d
            ),
            ExitCodes::Ok
        );
        assert_eq!(
            report(&Outcome::Disabled, "m", BackendKind::Disabled, d),
            ExitCodes::Ok
        );
        assert_eq!(
            report(
                &Outcome::Failed {
                    error: "backend: x".into()
                },
                "m",
                BackendKind::Remote,
                d
            ),
            ExitCodes::Generic
        );
    }
}
