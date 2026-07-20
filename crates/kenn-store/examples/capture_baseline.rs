//! Task 0.1 (replace-lance-with-sqlite): freeze the Lance ranking baseline as a
//! committed fixture, while the Lance backend still exists. The `SQLite` identifier/
//! BM25 parity gate (tasks 4.4 / 5.3) diffs against this.
//!
//! Model-free by construction: identifier search needs no embedding, and blended
//! search is captured with `query_vec = None` (lexical fusion only) so the fixture
//! is deterministic without the embedder. The vector arm is NOT baselined here —
//! per design D5/Risks it is exact (vs Lance's approximate `IVF_PQ`) and validated by
//! NN sanity, not by overlap with Lance.
//!
//! Run: `cargo run -p kenn-store --example capture_baseline`
//! Writes: `crates/kenn-store/tests/fixtures/lance_baseline.json`

use anyhow::{Context, Result};
use kenn_store::api::Reader;
use serde_json::json;

const SNAPSHOT: &str = ".kenn/local/live";
const OUT: &str = "crates/kenn-store/tests/fixtures/lance_baseline.json";

const QUERIES: &[&str] = &[
    "reader",
    "embed",
    "staleness",
    "search",
    "vector",
    "session",
    "hook",
    "parse",
    "snapshot",
    "lance",
    "sidecar",
    "watcher",
    "blended",
    "symbol",
    "schema",
    "ingest",
    "fingerprint",
    "collector",
    "graph",
    "reindex",
    "tokenize",
    "quant",
    "manifest",
    "package",
    "finding",
    "scan",
    "debounce",
    "cosine",
    "writer",
    "lifecycle",
];

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let reader = kenn_store::open_reader(std::path::Path::new(SNAPSHOT))
        .await
        .context("open_reader")?;

    let mut entries = Vec::new();
    for &q in QUERIES {
        let (by_name, _) = reader
            .search_symbols_by_name(q, 10, None, None, false, false)
            .await
            .with_context(|| format!("search_symbols_by_name({q})"))?;
        let blended = reader
            .search_symbols_blended(q, None, 10, false, false)
            .await
            .with_context(|| format!("search_symbols_blended({q})"))?;

        entries.push(json!({
            "q": q,
            "by_name": by_name.iter().map(|r| r.pub_id.clone()).collect::<Vec<_>>(),
            "blended": blended.iter().map(|r| r.symbol.pub_id.clone()).collect::<Vec<_>>(),
        }));
    }

    let fixture = json!({
        "backend": "lance",
        "note": "Lance top-k baseline; query_vec=None (lexical). Findings baseline omitted — this corpus has no findings.",
        "queries": entries,
    });

    if let Some(parent) = std::path::Path::new(OUT).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(OUT, serde_json::to_string_pretty(&fixture)? + "\n")?;

    let non_empty = entries
        .iter()
        .filter(|e| e["by_name"].as_array().is_some_and(|a| !a.is_empty()))
        .count();
    println!(
        "wrote {OUT}: {} queries ({non_empty} with by_name hits)",
        QUERIES.len()
    );
    Ok(())
}
