use super::*;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use kenn_embed::{EmbedError, EmbedKind, EmbeddingProducer, Loader};

use crate::host::{Host, HostConfig};

/// A deterministic fake producer — embeds every text as a vector
/// of `dim` floats where each component equals the input's length.
/// Token counts mirror character length / 4.
struct FakeProducer {
    dim: usize,
    inferences: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl EmbeddingProducer for FakeProducer {
    async fn embed(&self, texts: &[&str], _kind: EmbedKind) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.inferences.fetch_add(1, Ordering::SeqCst);
        #[expect(clippy::cast_precision_loss, reason = "test producer")]
        Ok(texts
            .iter()
            .map(|t| vec![t.len() as f32; self.dim])
            .collect())
    }
    fn dim(&self) -> usize {
        self.dim
    }
    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "trait signature is `-> &str`; impl returns a literal"
    )]
    fn model_id(&self) -> &str {
        "fake"
    }
}

fn fake_loader(inferences: Arc<AtomicUsize>) -> Loader {
    Arc::new(move || {
        Ok(Arc::new(FakeProducer {
            dim: 8,
            inferences: Arc::clone(&inferences),
        }) as Arc<dyn EmbeddingProducer>)
    })
}

/// Producer that emits a fixed vector of f32 values chosen to
/// stress the float-vs-base64 round-trip: small irrational-ish
/// fractions, subnormals, and edge values whose f32 and f64
/// nearest representations differ. If the float path's
/// ryu-shortest serialization or the client's f64→f32 cast lost
/// any bits, this would catch it.
struct TrickyProducer;
const TRICKY_VECTOR: [f32; 8] = [
    0.1_f32,
    1.0_f32 / 3.0,
    f32::EPSILON,
    f32::MIN_POSITIVE,
    -0.123_456_78_f32,
    1.234_567_8e10_f32,
    1.234_567_8e-10_f32,
    std::f32::consts::PI,
];

#[async_trait::async_trait]
impl EmbeddingProducer for TrickyProducer {
    async fn embed(&self, texts: &[&str], _kind: EmbedKind) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|_| TRICKY_VECTOR.to_vec()).collect())
    }
    fn dim(&self) -> usize {
        TRICKY_VECTOR.len()
    }
    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "trait signature is `-> &str`; impl returns a literal"
    )]
    fn model_id(&self) -> &str {
        "tricky"
    }
}

fn tricky_loader() -> Loader {
    Arc::new(|| Ok(Arc::new(TrickyProducer) as Arc<dyn EmbeddingProducer>))
}

/// Build a `Host` listening on an ephemeral port, with the
/// embeddings module wired against `loader`. Returns the bound
/// addr + a `JoinHandle` that drives serve, plus the PID-file path
/// so tests can confirm cleanup.
async fn spawn_test_host(
    loader: Loader,
    model_id: &str,
) -> (SocketAddr, tokio::task::JoinHandle<()>, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pid_path = tmp.path().join("server.pid");
    let dir_box: Box<tempfile::TempDir> = Box::new(tmp);
    let dir_ref: &'static tempfile::TempDir = Box::leak(dir_box);
    let _ = dir_ref; // keep alive

    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let pid_path_for_host = pid_path.clone();
    let model_id = model_id.to_owned();
    let handle = tokio::spawn(async move {
        let host = Host::new(HostConfig {
            addr,
            pid_path: pid_path_for_host,
            idle_timeout: Some(Duration::from_secs(5)),
        })
        .with_module(EmbeddingsModule::with_loader(model_id, loader));
        host.serve().await.unwrap();
    });

    // Give the host a moment to bind on the same addr.
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if reqwest::get(&format!("http://{addr}/healthz"))
            .await
            .is_ok()
        {
            return (addr, handle, pid_path);
        }
    }
    panic!("server never came up at {addr}");
}

#[tokio::test]
async fn single_string_embed_returns_one_vector() {
    let inferences = Arc::new(AtomicUsize::new(0));
    let (addr, handle, _pid) =
        spawn_test_host(fake_loader(Arc::clone(&inferences)), "fake-model").await;

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/v1/embeddings"))
        .json(&serde_json::json!({
            "model": "fake-model",
            "input": "hello",
            "encoding_format": "float"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list");
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["index"], 0);
    let emb = data[0]["embedding"].as_array().unwrap();
    assert_eq!(emb.len(), 8); // dim
                              // "hello" has 5 chars → each component == 5.0 per FakeProducer.
    assert!((emb[0].as_f64().unwrap() - 5.0).abs() < 1e-6);
    assert_eq!(body["model"], "fake-model");
    let prompt_tokens = body["usage"]["prompt_tokens"].as_u64().unwrap();
    assert!(prompt_tokens >= 1);
    assert_eq!(prompt_tokens, body["usage"]["total_tokens"]);
    handle.abort();
}

#[tokio::test]
async fn batch_embed_returns_vectors_in_input_order() {
    let inferences = Arc::new(AtomicUsize::new(0));
    let (addr, handle, _pid) =
        spawn_test_host(fake_loader(Arc::clone(&inferences)), "fake-model").await;

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/v1/embeddings"))
        .json(&serde_json::json!({
            "model": "fake-model",
            "input": ["a", "bb", "ccc"],
            "encoding_format": "float"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 3);
    for (i, expected_len) in [1.0, 2.0, 3.0].iter().enumerate() {
        assert_eq!(data[i]["index"], i);
        let emb = data[i]["embedding"].as_array().unwrap();
        assert!((emb[0].as_f64().unwrap() - expected_len).abs() < 1e-6);
    }
    handle.abort();
}

#[tokio::test]
async fn empty_input_array_is_400() {
    let inferences = Arc::new(AtomicUsize::new(0));
    let (addr, handle, _pid) =
        spawn_test_host(fake_loader(Arc::clone(&inferences)), "fake-model").await;

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/v1/embeddings"))
        .json(&serde_json::json!({ "model": "fake-model", "input": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "input_empty");
    // No inference should have run for an empty input.
    assert_eq!(inferences.load(Ordering::SeqCst), 0);
    handle.abort();
}

#[tokio::test]
async fn unknown_model_id_is_404() {
    let inferences = Arc::new(AtomicUsize::new(0));
    let (addr, handle, _pid) =
        spawn_test_host(fake_loader(Arc::clone(&inferences)), "fake-model").await;

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/v1/embeddings"))
        .json(&serde_json::json!({ "model": "does-not-exist", "input": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "model_not_found");
    // The body must reference both ids so the client can self-diagnose.
    let msg = body["error"]["message"].as_str().unwrap();
    assert!(msg.contains("does-not-exist") && msg.contains("fake-model"));
    // No inference for an unknown model.
    assert_eq!(inferences.load(Ordering::SeqCst), 0);
    handle.abort();
}

#[tokio::test]
async fn models_lists_exactly_the_configured_model() {
    let inferences = Arc::new(AtomicUsize::new(0));
    let (addr, handle, _pid) =
        spawn_test_host(fake_loader(Arc::clone(&inferences)), "fake-model").await;

    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("http://{addr}/v1/models"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["object"], "list");
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["id"], "fake-model");
    assert_eq!(data[0]["owned_by"], "kenn");
    // /v1/models must not touch the inference path.
    assert_eq!(inferences.load(Ordering::SeqCst), 0);
    handle.abort();
}

#[tokio::test]
async fn default_encoding_is_base64_not_float() {
    // Server default is `base64` (kenn deviation from OpenAI). A
    // request that omits `encoding_format` must come back as a
    // string, not a float array.
    let inferences = Arc::new(AtomicUsize::new(0));
    let (addr, handle, _pid) =
        spawn_test_host(fake_loader(Arc::clone(&inferences)), "fake-model").await;

    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("http://{addr}/v1/embeddings"))
        .json(&serde_json::json!({ "model": "fake-model", "input": "hello" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // The `embedding` field is a string when base64, an array when
    // float — assert the type-shape.
    assert!(
        body["data"][0]["embedding"].is_string(),
        "default encoding should be base64 (string), got {}",
        body["data"][0]["embedding"]
    );
    handle.abort();
}

#[tokio::test]
async fn float_and_base64_are_bit_identical_on_tricky_floats() {
    // Specifically stresses the float-vs-base64 round-trip on
    // values where f32 and f64 nearest representations differ
    // (0.1, 1/3, subnormals, π). The earlier
    // `float_and_base64_are_bit_identical_for_f32` test uses
    // FakeProducer (vectors of integer-valued floats that
    // trivially round-trip); this one exercises the part of the
    // contract that actually depends on ryu's f32-shortest
    // serialization being preserved through serde-json's f64
    // intermediate.
    let (addr, handle, _pid) = spawn_test_host(tricky_loader(), "tricky").await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/v1/embeddings");

    let float_body: serde_json::Value = client
        .post(&url)
        .json(&serde_json::json!({
            "model": "tricky", "input": "x", "encoding_format": "float"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let b64_body: serde_json::Value = client
        .post(&url)
        .json(&serde_json::json!({
            "model": "tricky", "input": "x", "encoding_format": "base64"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    #[expect(
        clippy::cast_possible_truncation,
        reason = "ryu-shortest f32 serialization round-trips through serde_json::Value::Number (f64) back to the original f32 — the cast here is the inverse of the widening serde performs at parse time"
    )]
    let from_float: Vec<f32> = float_body["data"][0]["embedding"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap() as f32)
        .collect();
    let from_b64: Vec<f32> =
        decode_base64(b64_body["data"][0]["embedding"].as_str().unwrap()).expect("decode base64");

    // Sanity: both paths surfaced the exact producer vector.
    assert_eq!(from_b64.len(), TRICKY_VECTOR.len());
    for (i, (got, expected)) in from_b64.iter().zip(&TRICKY_VECTOR).enumerate() {
        assert_eq!(
            got.to_bits(),
            expected.to_bits(),
            "base64 path: dim {i} expected {expected:?}, got {got:?}"
        );
    }
    // The actual bit-identity claim: float-encoded == base64-encoded.
    for (i, (a, b)) in from_float.iter().zip(&from_b64).enumerate() {
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "dim {i}: float-path {a} (bits {:#x}) != base64-path {b} (bits {:#x})",
            a.to_bits(),
            b.to_bits()
        );
    }
    handle.abort();
}

#[tokio::test]
async fn float_and_base64_are_bit_identical_for_f32() {
    // The same producer output, requested as `float` vs `base64`,
    // must produce bit-identical f32 vectors on the client side —
    // base64 carries raw bytes; the float path's JSON-emitted
    // numbers are ryu-shortest (round-trip exact for f32).
    let inferences = Arc::new(AtomicUsize::new(0));
    let (addr, handle, _pid) =
        spawn_test_host(fake_loader(Arc::clone(&inferences)), "fake-model").await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/v1/embeddings");

    let float_body: serde_json::Value = client
        .post(&url)
        .json(&serde_json::json!({
            "model": "fake-model",
            "input": "round-trip-probe",
            "encoding_format": "float"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let b64_body: serde_json::Value = client
        .post(&url)
        .json(&serde_json::json!({
            "model": "fake-model",
            "input": "round-trip-probe",
            "encoding_format": "base64"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // Parse both into Vec<f32> on the client side (NOT via f64).
    #[expect(
        clippy::cast_possible_truncation,
        reason = "JSON numbers serialized from f32 by ryu round-trip to the original f32; the f64-to-f32 cast is the lossless inverse of the f32-to-f64 widening serde does at parse time"
    )]
    let from_float: Vec<f32> = float_body["data"][0]["embedding"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap() as f32)
        .collect();
    let from_b64: Vec<f32> =
        decode_base64(b64_body["data"][0]["embedding"].as_str().unwrap()).expect("decode base64");
    assert_eq!(from_float.len(), from_b64.len());
    // Bit-identical f32 comparison — `to_bits()` to avoid NaN
    // surprises, but FakeProducer produces no NaNs.
    for (i, (a, b)) in from_float.iter().zip(&from_b64).enumerate() {
        assert_eq!(a.to_bits(), b.to_bits(), "dim {i}: float {a} != base64 {b}");
    }
    handle.abort();
}

#[tokio::test]
async fn base64_encoding_round_trips_to_float() {
    let inferences = Arc::new(AtomicUsize::new(0));
    let (addr, handle, _pid) =
        spawn_test_host(fake_loader(Arc::clone(&inferences)), "fake-model").await;

    let client = reqwest::Client::new();
    let body_float: serde_json::Value = client
        .post(format!("http://{addr}/v1/embeddings"))
        .json(&serde_json::json!({
            "model": "fake-model",
            "input": "test-input",
            "encoding_format": "float"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let body_b64: serde_json::Value = client
        .post(format!("http://{addr}/v1/embeddings"))
        .json(&serde_json::json!({
            "model": "fake-model",
            "input": "test-input",
            "encoding_format": "base64"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    #[expect(
        clippy::cast_possible_truncation,
        reason = "JSON numbers parse as f64; the producer-side values are f32 and round-trip without loss"
    )]
    let floats: Vec<f32> = body_float["data"][0]["embedding"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap() as f32)
        .collect();
    let b64_str = body_b64["data"][0]["embedding"].as_str().unwrap();
    let decoded = decode_base64(b64_str).expect("decode");
    assert_eq!(decoded.len(), floats.len());
    for (a, b) in floats.iter().zip(&decoded) {
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }
    handle.abort();
}

#[tokio::test]
async fn concurrent_requests_each_get_only_their_own_data() {
    // Confirms that coalescing preserves per-caller accounting:
    // request A asks for ["alpha"], request B asks for
    // ["beta", "gamma"] — each must see exactly its own inputs
    // even if the worker batched them into one inference call.
    let inferences = Arc::new(AtomicUsize::new(0));
    let (addr, handle, _pid) =
        spawn_test_host(fake_loader(Arc::clone(&inferences)), "fake-model").await;

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/v1/embeddings");
    let req_a = client
        .post(&url)
        .json(&serde_json::json!({
            "model": "fake-model", "input": "alpha", "encoding_format": "float"
        }))
        .send();
    let req_b = client
        .post(&url)
        .json(&serde_json::json!({
            "model": "fake-model", "input": ["beta", "gamma"], "encoding_format": "float"
        }))
        .send();

    let (resp_a, resp_b) = tokio::join!(req_a, req_b);
    let body_a: serde_json::Value = resp_a.unwrap().json().await.unwrap();
    let body_b: serde_json::Value = resp_b.unwrap().json().await.unwrap();

    let data_a = body_a["data"].as_array().unwrap();
    assert_eq!(data_a.len(), 1);
    // "alpha" → 5 chars → each component 5.0
    assert!((data_a[0]["embedding"][0].as_f64().unwrap() - 5.0).abs() < 1e-6);

    let data_b = body_b["data"].as_array().unwrap();
    assert_eq!(data_b.len(), 2);
    assert!((data_b[0]["embedding"][0].as_f64().unwrap() - 4.0).abs() < 1e-6); // "beta"
    assert!((data_b[1]["embedding"][0].as_f64().unwrap() - 5.0).abs() < 1e-6); // "gamma"

    handle.abort();
}
