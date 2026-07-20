//! Re-export shim for the embedding-producer boundary (now in
//! `kenn-embed`) plus the storage-owned sidecar format.
//!
//! `kenn-embed` holds the trait, the lazy wrapper, the in-process
//! `LlamaEmbedder`, and the process-global `shared_embedder()` —
//! both this crate (storage) and `kenn-server` (the daemon's
//! embeddings HTTP module) depend on it.
//!
//! The sidecar — `.kenn/vectors/manifest.toml` + segment files — is
//! storage-specific and stays here.

pub(crate) mod sidecar;

pub use kenn_embed::{
    init_shared_embedder, release_shared_embedder, shared_embedder, EmbedError, EmbeddingProducer,
    LazyEmbedder, LlamaEmbedder, Loader, SharedEmbedder, IDLE_TTL,
};

use crate::api::types::DbError;

impl From<EmbedError> for DbError {
    fn from(e: EmbedError) -> Self {
        match e {
            EmbedError::Io(io) => Self::Io(io),
            EmbedError::Backend(s) | EmbedError::Unreachable(s) => Self::Backend(s),
            EmbedError::Starting(s) => Self::EmbedderStarting(s),
        }
    }
}
