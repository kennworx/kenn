//! [`RemoteEmbedder`] — an [`EmbeddingProducer`] over an OpenAI-compatible
//! `/v1/embeddings` HTTP endpoint.
//!
//! Works against any provider speaking the `OpenAI` shape — kenn's own
//! server, `ollama`, `lm-studio`, hosted `OpenAI`. Per design D13, every
//! failure class (unreachable, non-2xx, malformed body, timeout) is
//! mapped to `EmbedError::Backend` so the wrapping [`LazyEmbedder`]
//! degrades to `Ok(None)` — search falls back to lexical-only rather
//! than failing.
//!
//! The client requests `encoding_format: "base64"` for compact wire
//! payloads (~3× smaller than JSON-float arrays) and transparently
//! handles either encoding in the response — providers that ignore
//! the field return float arrays, which we accept.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::producer::apply_task_prompt;

use super::{EmbedError, EmbedKind, EmbeddingProducer};

/// `EmbeddingProducer` over an OpenAI-compatible HTTP endpoint.
///
/// `base_url` is the provider root (e.g. `http://localhost:11434`);
/// the client appends `/v1/embeddings` and `/v1/models` itself.
/// Trailing slashes are tolerated.
pub struct RemoteEmbedder {
    base_url: String,
    model: String,
    client: reqwest::Client,
    /// Resolved at construction so `dim()` is sync. v1 hard-codes the
    /// `EmbeddingGemma` dimension (768) and verifies via a one-shot
    /// embed of a probe string at first use; v2 could call
    /// `GET /v1/models` to discover.
    dim: usize,
    /// Maximum inputs per `POST /v1/embeddings` request. A call to
    /// [`embed`](Self::embed) with more inputs than this is split into
    /// multiple back-to-back requests; the returned vectors are concatenated
    /// in input order. Sourced from `EmbeddingsConfig::batch_size`.
    batch_size: usize,
}

/// Fallback when `EmbeddingsConfig::batch_size` is zero. Matches the config
/// default in `kenn-config`.
const DEFAULT_BATCH_SIZE: usize = 256;

impl RemoteEmbedder {
    /// New embedder pointing at `base_url` requesting `model`. `batch_size`
    /// caps inputs per `POST /v1/embeddings` request — zero falls back to
    /// [`DEFAULT_BATCH_SIZE`].
    #[must_use]
    pub fn new(base_url: &str, model: &str, dim: usize, batch_size: usize) -> Self {
        let base_url = trim_trailing_slash(base_url).to_owned();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            base_url,
            model: model.to_owned(),
            client,
            dim,
            batch_size: if batch_size == 0 {
                DEFAULT_BATCH_SIZE
            } else {
                batch_size
            },
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{base}{path}", base = self.base_url)
    }

    fn embed_endpoint(&self) -> String {
        self.endpoint("/v1/embeddings")
    }
}

fn trim_trailing_slash(s: &str) -> &str {
    s.strip_suffix('/').unwrap_or(s)
}

#[async_trait::async_trait]
impl EmbeddingProducer for RemoteEmbedder {
    async fn embed(&self, texts: &[&str], kind: EmbedKind) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        // The task prompt is applied client-side: the OpenAI wire has no
        // kind field, so the server embeds whatever bytes arrive (raw).
        let prompted = apply_task_prompt(&self.model, kind, texts);
        let texts: Vec<&str> = match &prompted {
            Some(p) => p.iter().map(String::as_str).collect(),
            None => texts.to_vec(),
        };
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(self.batch_size) {
            out.extend(self.embed_one_request(chunk).await?);
        }
        Ok(out)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

impl RemoteEmbedder {
    /// One `POST /v1/embeddings` round-trip. `chunk` is at most `batch_size`
    /// inputs by construction (the [`embed`](Self::embed) loop guarantees
    /// it). Returns the vectors in input order for this chunk.
    async fn embed_one_request(&self, chunk: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let body = EmbedRequest {
            model: &self.model,
            input: chunk,
            // Always explicit even though kenn's own server defaults to
            // base64 — against ollama / lm-studio / OpenAI (which all
            // default to "float"), this flips them onto the bit-exact
            // base64 path. Do NOT remove as a "redundancy" cleanup.
            encoding_format: "base64",
        };
        let url = self.embed_endpoint();
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                log_failure(&url, &format!("send: {e}"));
                if e.is_connect() || e.is_timeout() {
                    // The remote endpoint never accepted bytes — daemon
                    // died between probe and request, DNS failed, etc.
                    // Caller can reselect a backend.
                    EmbedError::Unreachable(format!("POST {url}: {e}"))
                } else {
                    EmbedError::Backend(format!("POST {url}: {e}"))
                }
            })?;
        let status = resp.status();
        if !status.is_success() {
            let snippet = resp.text().await.unwrap_or_default();
            let truncated: String = snippet.chars().take(200).collect();
            log_failure(&url, &format!("HTTP {status}: {truncated}"));
            return Err(EmbedError::Backend(format!(
                "POST {url}: HTTP {status}: {truncated}"
            )));
        }
        let parsed: EmbedResponse = resp.json().await.map_err(|e| {
            log_failure(&url, &format!("parse: {e}"));
            EmbedError::Backend(format!("POST {url}: parse response: {e}"))
        })?;
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(chunk.len());
        for entry in parsed.data {
            let v = match entry.embedding {
                EmbeddingValue::Float(v) => v,
                EmbeddingValue::Base64(s) => decode_base64(&s).map_err(|e| {
                    EmbedError::Backend(format!("POST {url}: decode base64 embedding: {e}"))
                })?,
            };
            out.push(v);
        }
        Ok(out)
    }
}

fn log_failure(url: &str, detail: &str) {
    tracing::warn!(
        target: "kenn_embed::remote",
        url,
        detail,
        "RemoteEmbedder request failed; falling back to lexical-only"
    );
}

// ============== OpenAI wire shapes (subset we use) =================

#[derive(Debug, Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [&'a str],
    encoding_format: &'a str,
}

#[derive(Debug, Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedEntry>,
}

#[derive(Debug, Deserialize)]
struct EmbedEntry {
    embedding: EmbeddingValue,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum EmbeddingValue {
    Float(Vec<f32>),
    Base64(String),
}

fn decode_base64(s: &str) -> Result<Vec<f32>, String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(s.as_bytes())
        .map_err(|e| format!("{e}"))?;
    if bytes.len() % 4 != 0 {
        return Err(format!(
            "decoded byte length {} not a multiple of 4",
            bytes.len()
        ));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|c| <[u8; 4]>::try_from(c).map_or(0.0, f32::from_le_bytes))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_trailing_slash_strips_one() {
        assert_eq!(trim_trailing_slash("http://x:9/"), "http://x:9");
        assert_eq!(trim_trailing_slash("http://x:9"), "http://x:9");
    }

    #[test]
    fn endpoint_joins_correctly() {
        let e = RemoteEmbedder::new("http://x:9", "m", 768, 256);
        assert_eq!(e.embed_endpoint(), "http://x:9/v1/embeddings");
        let e = RemoteEmbedder::new("http://x:9/", "m", 768, 256);
        assert_eq!(e.embed_endpoint(), "http://x:9/v1/embeddings");
    }

    #[test]
    fn model_id_returns_configured_model() {
        let e = RemoteEmbedder::new("http://x:9", "my-model", 768, 256);
        assert_eq!(e.model_id(), "my-model");
    }

    #[test]
    fn decode_base64_round_trips() {
        use base64::Engine as _;
        let v = [1.0_f32, -2.5, 3.125];
        let mut bytes = Vec::new();
        for x in &v {
            bytes.extend_from_slice(&x.to_le_bytes());
        }
        let s = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let decoded = decode_base64(&s).unwrap();
        assert_eq!(decoded, v.to_vec());
    }

    #[test]
    fn decode_base64_rejects_odd_length() {
        use base64::Engine as _;
        let bad = base64::engine::general_purpose::STANDARD.encode([1, 2, 3]); // 3 bytes — not 4-aligned
        decode_base64(&bad).unwrap_err();
    }

    #[tokio::test]
    async fn unreachable_endpoint_returns_err() {
        // Bind a port, drop the listener — that port is now refused.
        // (Better than a hard-coded port that might collide.)
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let e = RemoteEmbedder::new(&format!("http://{addr}"), "m", 8, 256);
        let err = e.embed(&["hello"], EmbedKind::Document).await.unwrap_err();
        // Connection refused → Unreachable (not Backend) so the shared
        // embedder reselects rather than degrading permanently.
        assert!(matches!(err, EmbedError::Unreachable(_)), "{err:?}");
    }

    #[tokio::test]
    async fn empty_input_returns_empty_no_request() {
        // No HTTP call expected — short-circuits.
        let e = RemoteEmbedder::new("http://127.0.0.1:1", "m", 8, 256);
        assert_eq!(
            e.embed(&[], EmbedKind::Document).await.unwrap(),
            Vec::<Vec<f32>>::new()
        );
    }

    /// A minimal sync HTTP/1.1 mock — accepts `POST /v1/embeddings` calls and
    /// records how many inputs each request carried. `respond` decides what
    /// to return per request (None → HTTP 500). Worker thread is detached
    /// and exits when the test process tears down.
    struct MockServer {
        addr: std::net::SocketAddr,
        inputs_per_request: std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    }

    impl MockServer {
        fn start(respond: impl Fn(usize, usize) -> Option<String> + Send + Sync + 'static) -> Self {
            use std::io::{Read, Write};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(false).unwrap();
            let addr = listener.local_addr().unwrap();
            let inputs_per_request = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let recorded = std::sync::Arc::clone(&inputs_per_request);
            let respond = std::sync::Arc::new(respond);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { break };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut buf = Vec::with_capacity(4096);
                    let mut tmp = [0u8; 4096];
                    let mut content_length: Option<usize> = None;
                    let mut header_end: Option<usize> = None;
                    while header_end.is_none() {
                        let n = match stream.read(&mut tmp) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(idx) = find_double_crlf(&buf) {
                            header_end = Some(idx + 4);
                            let head = std::str::from_utf8(&buf[..idx]).unwrap_or("");
                            for line in head.split("\r\n") {
                                let lower = line.to_ascii_lowercase();
                                if let Some(v) = lower.strip_prefix("content-length:") {
                                    content_length = v.trim().parse().ok();
                                }
                            }
                        }
                    }
                    let Some(hend) = header_end else { continue };
                    let need = content_length.unwrap_or(0);
                    while buf.len() < hend + need {
                        let n = match stream.read(&mut tmp) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let body = &buf[hend..hend + need.min(buf.len() - hend)];
                    let body_str = std::str::from_utf8(body).unwrap_or("{}");
                    let parsed: serde_json::Value =
                        serde_json::from_str(body_str).unwrap_or(serde_json::Value::Null);
                    let inputs: Vec<String> = parsed
                        .get("input")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    let inputs_len = inputs.len();
                    let request_index = {
                        let mut v = recorded.lock().unwrap();
                        v.push(inputs);
                        v.len() - 1
                    };
                    let body = respond(request_index, inputs_len);
                    let resp = match body {
                        Some(b) => format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            b.len(),
                            b,
                        ),
                        None => "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned(),
                    };
                    drop(stream.write_all(resp.as_bytes()));
                }
            });
            Self {
                addr,
                inputs_per_request,
            }
        }

        fn requests(&self) -> Vec<usize> {
            self.inputs_per_request
                .lock()
                .unwrap()
                .iter()
                .map(Vec::len)
                .collect()
        }

        /// The raw input strings each request carried — what the provider
        /// would actually tokenize. Used by the prompt-application tests.
        fn request_inputs(&self) -> Vec<Vec<String>> {
            self.inputs_per_request.lock().unwrap().clone()
        }
    }

    fn find_double_crlf(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n")
    }

    /// Build an OpenAI-shaped response body with `n` synthetic vectors —
    /// each is `dim` zero floats, base64-encoded. We only assert on counts
    /// and ordering, not vector contents.
    fn synthetic_body(n: usize, dim: usize) -> String {
        use base64::Engine as _;
        let zero_vec = vec![0u8; dim * 4]; // dim f32-LE zeros
        let b64 = base64::engine::general_purpose::STANDARD.encode(&zero_vec);
        let data: Vec<serde_json::Value> = (0..n)
            .map(|i| {
                serde_json::json!({
                    "index": i,
                    "object": "embedding",
                    "embedding": b64,
                })
            })
            .collect();
        serde_json::json!({
            "object": "list",
            "data": data,
            "model": "m",
            "usage": { "prompt_tokens": 0, "total_tokens": 0 },
        })
        .to_string()
    }

    #[tokio::test]
    async fn large_batch_splits_into_chunked_requests() {
        // 600 inputs with batch_size = 256 → 3 requests of 256, 256, 88.
        let server = MockServer::start(|_idx, inputs_len| Some(synthetic_body(inputs_len, 8)));
        let e = RemoteEmbedder::new(&format!("http://{}", server.addr), "m", 8, 256);
        let inputs: Vec<String> = (0..600).map(|i| format!("t{i}")).collect();
        let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        let out = e.embed(&refs, EmbedKind::Document).await.unwrap();
        assert_eq!(out.len(), 600);
        assert_eq!(server.requests(), vec![256, 256, 88]);
    }

    #[tokio::test]
    async fn chunk_failure_aborts_and_emits_no_partial() {
        // First request OK, second 500 → call returns Err and no third request.
        let server = MockServer::start(|idx, inputs_len| {
            if idx == 0 {
                Some(synthetic_body(inputs_len, 8))
            } else {
                None
            }
        });
        let e = RemoteEmbedder::new(&format!("http://{}", server.addr), "m", 8, 256);
        let inputs: Vec<String> = (0..600).map(|i| format!("t{i}")).collect();
        let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        let err = e.embed(&refs, EmbedKind::Document).await.unwrap_err();
        assert!(matches!(err, EmbedError::Backend(_)), "{err:?}");
        // Two requests issued (the first succeeded, the second failed); no third.
        let req = server.requests();
        assert!(
            req.len() == 2 && req[0] == 256 && req[1] == 256,
            "expected exactly two requests of 256, got {req:?}"
        );
    }

    #[tokio::test]
    async fn single_chunk_input_emits_one_request() {
        let server = MockServer::start(|_idx, inputs_len| Some(synthetic_body(inputs_len, 8)));
        let e = RemoteEmbedder::new(&format!("http://{}", server.addr), "m", 8, 256);
        let inputs = ["a", "b", "c"];
        let out = e.embed(&inputs, EmbedKind::Document).await.unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(server.requests(), vec![3]);
    }

    #[tokio::test]
    async fn query_kind_prepends_the_gemma_prompt_on_the_wire() {
        let server = MockServer::start(|_idx, inputs_len| Some(synthetic_body(inputs_len, 8)));
        let e = RemoteEmbedder::new(
            &format!("http://{}", server.addr),
            "embeddinggemma-300M",
            8,
            256,
        );
        e.embed(&["find the parser"], EmbedKind::Query)
            .await
            .unwrap();
        assert_eq!(
            server.request_inputs(),
            vec![vec![
                "task: search result | query: find the parser".to_owned()
            ]]
        );
    }

    #[tokio::test]
    async fn document_kind_sends_raw_text_for_gemma() {
        // The doc prompt is deferred — corpus texts must reach the provider
        // byte-identical so stored vectors need no invalidation.
        let server = MockServer::start(|_idx, inputs_len| Some(synthetic_body(inputs_len, 8)));
        let e = RemoteEmbedder::new(
            &format!("http://{}", server.addr),
            "embeddinggemma-300M",
            8,
            256,
        );
        e.embed(&["fn parse()"], EmbedKind::Document).await.unwrap();
        assert_eq!(server.request_inputs(), vec![vec!["fn parse()".to_owned()]]);
    }

    #[tokio::test]
    async fn non_gemma_model_sends_raw_text_for_both_kinds() {
        let server = MockServer::start(|_idx, inputs_len| Some(synthetic_body(inputs_len, 8)));
        let e = RemoteEmbedder::new(
            &format!("http://{}", server.addr),
            "nomic-embed-text",
            8,
            256,
        );
        e.embed(&["hello"], EmbedKind::Query).await.unwrap();
        e.embed(&["hello"], EmbedKind::Document).await.unwrap();
        assert_eq!(
            server.request_inputs(),
            vec![vec!["hello".to_owned()], vec!["hello".to_owned()]]
        );
    }

    #[test]
    fn zero_batch_size_falls_back_to_default() {
        let e = RemoteEmbedder::new("http://x:1", "m", 8, 0);
        assert_eq!(e.batch_size, DEFAULT_BATCH_SIZE);
    }
}
