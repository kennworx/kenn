//! `kenn-embed` — the embedding-producer boundary, the lazily-loaded
//! producer wrapper, and the in-process `LlamaEmbedder` impl.
//!
//! Extracted from `kenn-store::embed` so that both `kenn-store` (the
//! storage layer's consumer of embeddings) and `kenn-server` (the
//! daemon that hosts the embeddings HTTP API) can depend on the same
//! types without one having to depend on the other.
//!
//! [`EmbeddingProducer`] is the one trait every implementation
//! satisfies — currently [`LlamaEmbedder`] (in-process llama.cpp) and
//! [`RemoteEmbedder`] (HTTP). [`LazyEmbedder`] wraps a producer with
//! on-demand loading and an idle TTL.

pub mod llama;
pub mod remote;
pub mod scheduler;
pub mod spawn;

mod lazy;
mod producer;
mod selector;
mod shared;

pub use lazy::LazyEmbedder;
pub use llama::LlamaEmbedder;
pub use producer::{EmbedError, EmbedKind, EmbeddingProducer, Loader, DEFAULT_EMBED_DIM, IDLE_TTL};
pub use remote::RemoteEmbedder;
pub use shared::{
    init_shared_embedder, release_shared_embedder, shared_embedder, BackendKind, SharedEmbedder,
};
