//! `kenn-server` — the per-user kenn daemon.
//!
//! A long-lived HTTP host that workspace-local kenn invocations
//! (`kenn mcp`, `kenn index`, etc.) talk to over loopback. The host is a
//! tiny `axum`-based service ([`Host`]) plus N pluggable
//! [`Module`]s. Each module registers its own routes; the host owns the
//! listener, the PID file, `/healthz`, the idle-timeout aggregator, and
//! graceful shutdown.
//!
//! v1 ships one capability — OpenAI-compatible embeddings (a follow-up
//! commit wires the embeddings module in).

pub mod embeddings;
pub mod host;
pub mod paths;
pub mod pid;
pub mod runtime;

pub use embeddings::EmbeddingsModule;
pub use host::{Host, HostConfig, Module};

/// Errors surfaced by `kenn-server` operations.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("config: {0}")]
    Config(#[from] kenn_config::ConfigError),
    #[error("bind {addr}: {source}")]
    Bind {
        addr: String,
        #[source]
        source: std::io::Error,
    },
    #[error("no state directory could be resolved for this user")]
    NoStateDir,
    #[error("daemonization failed: {0}")]
    Daemon(String),
    #[error("server: {0}")]
    Other(String),
}

/// Convenience `Result` alias.
pub type Result<T, E = ServerError> = std::result::Result<T, E>;
