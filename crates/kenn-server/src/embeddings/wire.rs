//! OpenAI-compatible wire shapes for the embeddings + models endpoints,
//! plus the priority-override header constant.

use serde::{Deserialize, Serialize};

/// Custom HTTP header callers MAY use to override the cardinality-derived
/// priority (`interactive` = high, `bulk` = low). See embed-daemon-streaming.
pub(crate) const PRIORITY_HEADER: &str = "X-Kenn-Priority";

// ============== OpenAI request / response shapes ===================

#[derive(Debug, Deserialize)]
pub struct EmbeddingsRequest {
    pub model: String,
    pub input: Input,
    #[serde(default)]
    pub encoding_format: EncodingFormat,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Input {
    One(String),
    Many(Vec<String>),
}

impl Input {
    pub(crate) fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(s) => vec![s],
            Self::Many(v) => v,
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum EncodingFormat {
    Float,
    /// **kenn's default** when the request omits `encoding_format`.
    /// Bit-exact (no f32 → JSON-number rounding) and ~3× smaller on
    /// the wire than `Float`. This is a deliberate deviation from the
    /// `OpenAI` default of `"float"`; clients that need float arrays
    /// must send `encoding_format: "float"` explicitly.
    #[default]
    Base64,
}

#[derive(Debug, Serialize)]
pub struct EmbeddingsResponse {
    pub object: &'static str, // "list"
    pub data: Vec<EmbeddingEntry>,
    pub model: String,
    pub usage: Usage,
}

#[derive(Debug, Serialize)]
pub struct EmbeddingEntry {
    pub object: &'static str, // "embedding"
    pub index: usize,
    pub embedding: EmbeddingValue,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum EmbeddingValue {
    Float(Vec<f32>),
    Base64(String),
}

#[derive(Debug, Serialize)]
pub struct Usage {
    pub prompt_tokens: usize,
    pub total_tokens: usize,
}

#[derive(Debug, Serialize)]
pub struct ModelsResponse {
    pub object: &'static str, // "list"
    pub data: Vec<ModelEntry>,
}

#[derive(Debug, Serialize)]
pub struct ModelEntry {
    pub id: String,
    pub object: &'static str,   // "model"
    pub owned_by: &'static str, // "kenn"
}
