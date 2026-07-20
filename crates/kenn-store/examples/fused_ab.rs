//! Fused-hybrid A/B check for `embedding-gemma-prompts` (tasks.md §2.4): does
//! the query task prompt regress the SHIPPED search path — the RRF-blended
//! lexical + vector `search_symbols_blended` — on the self-supervised gold set?
//!
//! Two arms over the same committed index (the live snapshot and its
//! committed vectors); only the query vector differs:
//!   raw      = query embedded as `Document` kind (no prompt — old behavior)
//!   prompted = query embedded as `Query` kind (the shipped query prompt)
//!
//! Gold mirrors `prompt_ab`: the query is a documented symbol's first doc
//! line; the target is that symbol. The query derives from the doc, so
//! absolute recall is inflated — the *delta between arms* is what matters.
//!
//! Usage: `cargo run -p kenn-store --example fused_ab -- [live-snapshot-dir]`
//! Env overrides: `AB_N` (gold size, default 200).

use std::path::{Path, PathBuf};

use kenn_embed::{EmbedKind, EmbeddingProducer, LlamaEmbedder};
use kenn_store::api::Reader;
use rusqlite::{Connection, OpenFlags};

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "SQLite ids are small non-negative integers"
)]
fn u32c(v: i64) -> u32 {
    v as u32
}
#[expect(
    clippy::cast_precision_loss,
    reason = "metric counts are well under 2^52"
)]
fn f64c(v: usize) -> f64 {
    v as f64
}

#[expect(clippy::panic, reason = "eval rig: fail fast on a bad snapshot path")]
fn open_ro(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap_or_else(|e| panic!("open {} read-only: {e}", path.display()))
}

/// First non-empty line of the raw doc, whitespace-collapsed, capped at 160
/// chars — the self-supervised "conceptual query" (matches `prompt_ab`).
fn first_line(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

/// Gold pairs `(query, target symbol id)` from the snapshot's documented,
/// non-external, non-test symbols (same filter as `prompt_ab`).
fn load_gold(g: &Connection, n: usize) -> Vec<(String, u32)> {
    let mut s = g
        .prepare(
            "SELECT d.sym_id, d.doc FROM symbol_docs d JOIN symbols s ON s.id=d.sym_id \
             WHERE s.external=0 AND s.test=0 AND length(d.doc)>=60 ORDER BY d.sym_id",
        )
        .expect("prepare");
    let all: Vec<(u32, String)> = s
        .query_map([], |r| Ok((u32c(r.get(0)?), r.get::<_, String>(1)?)))
        .expect("query")
        .map(|r| r.expect("row"))
        .filter_map(|(id, raw)| {
            let query = first_line(&raw);
            (!query.is_empty()).then_some((id, query))
        })
        .collect();
    let stride = (all.len() / n).max(1);
    all.iter()
        .step_by(stride)
        .take(n)
        .map(|(id, q)| (q.clone(), *id))
        .collect()
}

/// Embed every gold query with `kind` (batched, order-preserving).
fn embed_queries(
    e: &LlamaEmbedder,
    rt: &tokio::runtime::Runtime,
    gold: &[(String, u32)],
    kind: EmbedKind,
) -> Vec<Vec<f32>> {
    let refs: Vec<&str> = gold.iter().map(|(q, _)| q.as_str()).collect();
    let mut out = Vec::with_capacity(refs.len());
    for chunk in refs.chunks(32) {
        out.extend(rt.block_on(e.embed(chunk, kind)).expect("embed queries"));
    }
    out
}

/// Fused recall of `gold` through `search_symbols_blended` with the given
/// per-query vectors. Returns (recall@1, recall@10, MRR@10).
fn eval_fused(
    reader: &impl Reader,
    rt: &tokio::runtime::Runtime,
    gold: &[(String, u32)],
    qvs: &[Vec<f32>],
) -> (f64, f64, f64) {
    let (mut r1, mut r10, mut mrr) = (0usize, 0usize, 0.0f64);
    for ((query, target), qv) in gold.iter().zip(qvs) {
        let hits = rt
            .block_on(reader.search_symbols_blended(query, Some(qv), 10, false, false))
            .expect("blended search");
        if let Some(p) = hits.iter().position(|h| h.symbol.id == *target) {
            if p < 1 {
                r1 += 1;
            }
            r10 += 1; // target_total = 10, so any hit position is < 10
            mrr += 1.0 / f64c(p + 1);
        }
    }
    let n = f64c(gold.len().max(1));
    (f64c(r1) / n, f64c(r10) / n, mrr / n)
}

fn main() {
    let snap = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| ".kenn/local/live".into()),
    );
    let snap = std::fs::canonicalize(&snap).expect("canonicalize snapshot dir");
    let n: usize = std::env::var("AB_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");

    eprintln!("snapshot = {}", snap.display());
    // `vec_knowledge` itself needs the vec0 module; its `_rowids` shadow
    // table is plain SQLite and countable from this raw connection.
    let committed: i64 = open_ro(&snap.join("vector.db"))
        .query_row("SELECT count(*) FROM vec_knowledge_rowids", [], |r| {
            r.get(0)
        })
        .expect("count committed vectors");
    eprintln!("committed vectors = {committed}");
    assert!(
        committed > 0,
        "no committed vectors — run the embedding pass first or the vector arm is empty"
    );

    let gold = load_gold(&open_ro(&snap.join("code.db")), n);
    eprintln!("gold queries = {}", gold.len());

    eprintln!("loading embedder…");
    let e = LlamaEmbedder::load("embeddinggemma-300M".to_owned()).expect("load embedder");
    let reader = rt.block_on(kenn_store::open_reader(&snap)).expect("reader");

    eprintln!("embedding queries (raw / Document kind)…");
    let raw = embed_queries(&e, &rt, &gold, EmbedKind::Document);
    eprintln!("embedding queries (prompted / Query kind)…");
    let prompted = embed_queries(&e, &rt, &gold, EmbedKind::Query);

    let base = eval_fused(&reader, &rt, &gold, &raw);
    let new = eval_fused(&reader, &rt, &gold, &prompted);

    println!(
        "\n{:<28} {:>8} {:>8} {:>8}",
        "arm (fused hybrid)", "r@1", "r@10", "mrr@10"
    );
    println!("{}", "-".repeat(56));
    println!(
        "{:<28} {:>8.3} {:>8.3} {:>8.3}",
        "raw query (baseline)", base.0, base.1, base.2
    );
    println!(
        "{:<28} {:>8.3} {:>8.3} {:>8.3}",
        "prompted query (shipped)", new.0, new.1, new.2
    );
    println!(
        "\nprompted − raw:  Δr@1={:+.3}  Δr@10={:+.3}  Δmrr={:+.3}",
        new.0 - base.0,
        new.1 - base.1,
        new.2 - base.2
    );
    println!("gate (tasks.md §2.4): fused r@10 must not regress.");
}
