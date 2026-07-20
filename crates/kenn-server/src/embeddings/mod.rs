//! OpenAI-compatible embeddings capability for the kenn-server host
//! (specs/embeddings-api).
//!
//! Exposes `POST /v1/embeddings` and `GET /v1/models`. Concurrent
//! single-string requests are coalesced into one llama.cpp batch by a
//! single inference worker (design D10).
//!
//! Module layout:
//! - [`wire`] — the request/response wire shapes.
//! - [`module`] — the `EmbeddingsModule` host module + scheduler wiring.
//! - [`handlers`] — the HTTP handlers + priority/codec helpers.

mod handlers;
mod module;
mod wire;

#[cfg(test)]
mod tests;

pub use handlers::decode_base64;
pub use module::EmbeddingsModule;
pub use wire::{
    EmbeddingEntry, EmbeddingValue, EmbeddingsRequest, EmbeddingsResponse, EncodingFormat, Input,
    ModelEntry, ModelsResponse, Usage,
};

pub(crate) use handlers::{embeddings_handler, models_handler};
pub(crate) use wire::PRIORITY_HEADER;
