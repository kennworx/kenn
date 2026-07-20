//! Integration test for the in-process llama embedder. Loads the real
//! `EmbeddingGemma-300M` GGUF, runs `embed`, and asserts on the
//! structural properties of the output. See
//! `openspec/changes/kenn-embed-integration-test/` for rationale.
//!
//! `#[ignore]`'d by default — first run downloads ~300MB of weights.
//! Invoke via `just embed-smoke`.

#![cfg(target_os = "macos")]

use kenn_embed::{EmbedKind, EmbeddingProducer, LlamaEmbedder};

fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

#[tokio::test]
#[ignore = "downloads ~300MB GGUF on first run; invoke via `just embed-smoke`"]
async fn llama_embedder_produces_normalized_vectors() {
    let embedder =
        LlamaEmbedder::load("embeddinggemma-300M".to_owned()).expect("load embedding model");
    let dim = embedder.dim();

    let inputs = ["hello world", "the quick brown fox"];
    let vectors = embedder
        .embed(&inputs, EmbedKind::Document)
        .await
        .expect("embed inputs");

    assert_eq!(vectors.len(), inputs.len(), "one vector per input");
    for (i, v) in vectors.iter().enumerate() {
        assert_eq!(v.len(), dim, "vector {i} dim matches producer.dim()");
        let n = l2_norm(v);
        assert!(
            (n - 1.0).abs() < 1e-3,
            "vector {i} should be L2-normalized, got norm={n}",
        );
        assert!(
            v.iter().any(|x| *x != 0.0),
            "vector {i} should not be all-zero",
        );
    }
    assert_ne!(
        vectors[0], vectors[1],
        "distinct inputs should produce distinct vectors",
    );
}

/// The query task prompt is applied on the query side only
/// (`embedding-gemma-prompts`): the same text embedded as a query vs a
/// document must produce different vectors, and the query-kind vector must
/// equal a document-kind embed of the manually-prompted text (the prompt
/// is a plain prefix at tokenize time — nothing else differs).
#[tokio::test]
#[ignore = "downloads ~300MB GGUF on first run; invoke via `just embed-smoke`"]
async fn query_kind_prompts_and_document_kind_stays_raw() {
    let embedder =
        LlamaEmbedder::load("embeddinggemma-300M".to_owned()).expect("load embedding model");

    let text = "resolve the workspace root";
    let as_query = embedder
        .embed(&[text], EmbedKind::Query)
        .await
        .expect("embed query");
    let as_document = embedder
        .embed(&[text], EmbedKind::Document)
        .await
        .expect("embed document");
    assert_ne!(
        as_query[0], as_document[0],
        "the query prompt must change the vector",
    );

    let manually_prompted = format!("task: search result | query: {text}");
    let as_manual_document = embedder
        .embed(&[manually_prompted.as_str()], EmbedKind::Document)
        .await
        .expect("embed manually prompted");
    assert_eq!(
        as_query[0], as_manual_document[0],
        "query kind must be exactly a prefix-prompted document embed",
    );
}
