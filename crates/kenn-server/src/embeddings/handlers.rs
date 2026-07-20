//! HTTP handlers for `/v1/embeddings` and `/v1/models`, request priority
//! classification, response building, and the base64 vector codec.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde_json::json;

use kenn_embed::scheduler::Priority;
use kenn_embed::EmbedKind;

use super::{
    EmbeddingEntry, EmbeddingValue, EmbeddingsModule, EmbeddingsRequest, EmbeddingsResponse,
    EncodingFormat, Input, ModelEntry, ModelsResponse, Usage, PRIORITY_HEADER,
};

// ============== HTTP handlers ======================================

pub(crate) async fn embeddings_handler(
    State(module): State<Arc<EmbeddingsModule>>,
    headers: HeaderMap,
    Json(req): Json<EmbeddingsRequest>,
) -> Response {
    if req.model != module.model_id {
        let msg = format!(
            "model `{}` not served by this kenn server (configured: `{}`)",
            req.model, module.model_id
        );
        return error_response(
            StatusCode::NOT_FOUND,
            "not_found_error",
            "model_not_found",
            Some("model"),
            &msg,
        );
    }

    // Classify priority from input shape (default) + optional override
    // header — see embed-daemon-streaming.
    let priority = classify_priority(&req.input, &headers);

    let inputs = req.input.into_vec();
    if inputs.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "input_empty",
            Some("input"),
            "`input` must contain at least one string",
        );
    }

    // Estimate per-input token counts for `usage`. The scheduler holds the
    // model on its worker thread; an exact tokenize would need to route
    // through it. The char-based estimate matches the EmbeddingProducer
    // trait default; integration tests rely on the same estimate.
    let counts: Vec<usize> = inputs.iter().map(|t| (t.len() / 4).max(1)).collect();

    let vectors = match dispatch(&module, inputs, priority).await {
        Ok(v) => v,
        Err((status, msg)) => {
            return error_response(status, "service_unavailable", "embedder_failed", None, &msg);
        }
    };

    Json(build_response(
        &module.model_id,
        vectors,
        &counts,
        req.encoding_format,
    ))
    .into_response()
}

/// Classify a request's priority. The standard `OpenAI` `input` shape decides
/// by default — `Input::One` is a one-shot query (high), `Input::Many` is a
/// batch (low). An optional `X-Kenn-Priority` header overrides; unrecognized
/// values fall back to cardinality. See `embed-daemon-streaming` D1.
fn classify_priority(input: &Input, headers: &HeaderMap) -> Priority {
    if let Some(value) = headers
        .get(PRIORITY_HEADER)
        .and_then(|h| h.to_str().ok())
        .map(str::trim)
    {
        match value.to_ascii_lowercase().as_str() {
            "interactive" => return Priority::High,
            "bulk" => return Priority::Low,
            _ => {} // fall through to cardinality
        }
    }
    match input {
        Input::One(_) => Priority::High,
        Input::Many(_) => Priority::Low,
    }
}

/// Submit a batch through the priority scheduler. Maps scheduler / load
/// failures into `(status, message)` pairs the handler serializes.
async fn dispatch(
    module: &EmbeddingsModule,
    inputs: Vec<String>,
    priority: Priority,
) -> Result<Vec<Vec<f32>>, (StatusCode, String)> {
    // Document kind = raw pass-through: the OpenAI wire carries no
    // query-vs-document field, so clients apply any task prompt before
    // sending and the daemon embeds exactly the bytes that arrived
    // (prompting here would double-prompt `RemoteEmbedder` queries).
    module
        .scheduler
        .submit(inputs, priority, EmbedKind::Document)
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, format!("{e}")))
}

/// Build the OpenAI-shaped response body.
fn build_response(
    model_id: &str,
    vectors: Vec<Vec<f32>>,
    counts: &[usize],
    encoding: EncodingFormat,
) -> EmbeddingsResponse {
    let total_tokens: usize = counts.iter().sum();
    let data = vectors
        .into_iter()
        .enumerate()
        .map(|(index, v)| EmbeddingEntry {
            object: "embedding",
            index,
            embedding: match encoding {
                EncodingFormat::Float => EmbeddingValue::Float(v),
                EncodingFormat::Base64 => EmbeddingValue::Base64(encode_base64(&v)),
            },
        })
        .collect();
    EmbeddingsResponse {
        object: "list",
        data,
        model: model_id.to_owned(),
        usage: Usage {
            prompt_tokens: total_tokens,
            total_tokens,
        },
    }
}

/// Compose an OpenAI-shaped error response with the given status code.
fn error_response(
    status: StatusCode,
    err_type: &'static str,
    code: &'static str,
    param: Option<&'static str>,
    message: &str,
) -> Response {
    let mut error = serde_json::Map::new();
    error.insert("message".into(), json!(message));
    error.insert("type".into(), json!(err_type));
    error.insert("code".into(), json!(code));
    if let Some(p) = param {
        error.insert("param".into(), json!(p));
    }
    (
        status,
        Json(serde_json::Value::Object(serde_json::Map::from_iter([(
            "error".into(),
            serde_json::Value::Object(error),
        )]))),
    )
        .into_response()
}

pub(crate) async fn models_handler(
    State(module): State<Arc<EmbeddingsModule>>,
) -> Json<ModelsResponse> {
    Json(ModelsResponse {
        object: "list",
        data: vec![ModelEntry {
            id: module.model_id.clone(),
            object: "model",
            owned_by: "kenn",
        }],
    })
}

/// Encode an f32 vector as base64 of its little-endian byte
/// representation (the `OpenAI` / `ollama` convention).
fn encode_base64(v: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for x in v {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    B64.encode(&bytes)
}

/// Decode the base64 form back into f32-LE — exposed for the
/// `RemoteEmbedder` (§4) and the integration test, so the two sides
/// agree on the encoding.
pub fn decode_base64(s: &str) -> Result<Vec<f32>, base64::DecodeError> {
    let bytes = B64.decode(s.as_bytes())?;
    // `chunks_exact(4)` yields slices of exactly 4 bytes — split into
    // a 4-byte array via `try_into` to avoid the indexing-may-panic
    // lint. The infallible array conversion is folded into the iterator.
    Ok(bytes
        .chunks_exact(4)
        .map(|c| <[u8; 4]>::try_from(c).map_or(0.0, f32::from_le_bytes))
        .collect())
}
