//! A/B eval (gate for the `embedding-gemma-prompts` change): does `EmbeddingGemma`'s
//! asymmetric task prompt improve kenn's conceptual doc→symbol *vector* recall?
//!
//! Isolated vector arm (no FTS/fusion), three arms over the same gold set:
//!   none       = raw doc, raw query          — current kenn behavior (baseline)
//!   query-only = raw doc, prompted query
//!   query+doc  = prompted doc, prompted query — the proposed behavior
//!
//! Prompts (confirmed against the model card / SHIFT paper):
//!   query    prefix = "task: search result | query: "
//!   document prefix = "title: none | text: "
//!
//! Gold is self-supervised, mirroring `composed_spike`'s G2: the query is a
//! documented symbol's first doc line; the target is that symbol; the doc-only
//! index holds every documented symbol (the rest are distractors). The query is
//! derived from the doc, so absolute recall is inflated — but the *delta between
//! arms* is fair (every arm shares the leakage). We report the delta, not the level.
//!
//! Usage: `cargo run -p kenn-store --example prompt_ab -- [snapshot-code.db-dir]`
//! Env overrides: `AB_N` (gold size, default 200), `AB_QPROMPT`, `AB_DPROMPT`.

use std::path::{Path, PathBuf};

use kenn_embed::{EmbedKind, EmbeddingProducer, LlamaEmbedder};
use rusqlite::{Connection, OpenFlags};

const Q_PROMPT: &str = "task: search result | query: ";
const D_PROMPT: &str = "title: none | text: ";

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

/// Strip XML/HTML tags (C# `<summary>` docs) and collapse whitespace — matches
/// `composed_spike`'s `clean_doc` so the baseline reflects what kenn stores.
fn clean_doc(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for c in raw.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// First non-empty line of the raw doc, whitespace-collapsed, capped at 160 chars
/// — the self-supervised "conceptual query" for a symbol (`composed_spike`'s G2).
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

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum() // vectors are L2-normalized → dot = cosine
}

/// Embed `texts` (each with `prefix` prepended) in batches, preserving order.
fn embed_all(
    e: &LlamaEmbedder,
    rt: &tokio::runtime::Runtime,
    prefix: &str,
    texts: &[String],
) -> Vec<Vec<f32>> {
    let prefixed: Vec<String> = texts.iter().map(|t| format!("{prefix}{t}")).collect();
    let refs: Vec<&str> = prefixed.iter().map(String::as_str).collect();
    let mut out = Vec::with_capacity(texts.len());
    // Document kind = raw text: this harness applies its own arm-specific
    // prefixes, so the producer's built-in query prompting must stay out.
    for chunk in refs.chunks(32) {
        out.extend(
            rt.block_on(e.embed(chunk, EmbedKind::Document))
                .expect("embed batch"),
        );
    }
    out
}

/// A documented symbol: its id, its cleaned full doc (the index text), and its
/// first doc line (the query text).
struct Documented {
    id: u32,
    doc: String,
    query: String,
}

fn load_documented(g: &Connection) -> Vec<Documented> {
    let mut s = g
        .prepare(
            "SELECT d.sym_id, d.doc FROM symbol_docs d JOIN symbols s ON s.id=d.sym_id \
             WHERE s.external=0 AND s.test=0 AND length(d.doc)>=60 ORDER BY d.sym_id",
        )
        .expect("prepare");
    s.query_map([], |r| Ok((u32c(r.get(0)?), r.get::<_, String>(1)?)))
        .expect("query")
        .map(|r| r.expect("row"))
        .filter_map(|(id, raw)| {
            let doc = clean_doc(&raw);
            let query = first_line(&raw);
            (doc.len() >= 30 && !query.is_empty()).then_some(Documented { id, doc, query })
        })
        .collect()
}

/// Vector-arm-only recall of `gold` targets against the doc index `(ids, vecs)`.
/// Queries are embedded with `q_prefix`. Returns (recall@1, recall@10, MRR).
fn eval_vec(
    e: &LlamaEmbedder,
    rt: &tokio::runtime::Runtime,
    ids: &[u32],
    vecs: &[Vec<f32>],
    gold: &[(String, u32)],
    q_prefix: &str,
) -> (f64, f64, f64) {
    let queries: Vec<String> = gold.iter().map(|(q, _)| q.clone()).collect();
    let qvs = embed_all(e, rt, q_prefix, &queries);
    let (mut r1, mut r10, mut mrr) = (0usize, 0usize, 0.0f64);
    for ((_, target), qv) in gold.iter().zip(&qvs) {
        let mut scored: Vec<(u32, f32)> = ids
            .iter()
            .zip(vecs)
            .map(|(id, v)| (*id, cosine(qv, v)))
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        if let Some(p) = scored.iter().position(|(id, _)| id == target) {
            if p < 1 {
                r1 += 1;
            }
            if p < 10 {
                r10 += 1;
            }
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
    let g = open_ro(&snap.join("code.db"));
    let q_prompt = std::env::var("AB_QPROMPT").unwrap_or_else(|_| Q_PROMPT.to_owned());
    let d_prompt = std::env::var("AB_DPROMPT").unwrap_or_else(|_| D_PROMPT.to_owned());
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
    eprintln!("loading embedder…");
    let e = LlamaEmbedder::load("embeddinggemma-300M".to_owned()).expect("load embedder");

    let docs = load_documented(&g);
    eprintln!("documented symbols (index corpus) = {}", docs.len());
    let ids: Vec<u32> = docs.iter().map(|d| d.id).collect();
    let doc_texts: Vec<String> = docs.iter().map(|d| d.doc.clone()).collect();

    // Gold = strided subset of the indexed docs (guaranteed present in the index).
    let stride = (docs.len() / n).max(1);
    let gold: Vec<(String, u32)> = docs
        .iter()
        .step_by(stride)
        .take(n)
        .map(|d| (d.query.clone(), d.id))
        .collect();
    eprintln!("gold queries = {}", gold.len());

    eprintln!("embedding doc index (raw)…");
    let docs_raw = embed_all(&e, &rt, "", &doc_texts);
    eprintln!("embedding doc index (prompted)…");
    let docs_prompted = embed_all(&e, &rt, &d_prompt, &doc_texts);

    let none = eval_vec(&e, &rt, &ids, &docs_raw, &gold, "");
    let qonly = eval_vec(&e, &rt, &ids, &docs_raw, &gold, &q_prompt);
    let qdoc = eval_vec(&e, &rt, &ids, &docs_prompted, &gold, &q_prompt);

    println!("\n{:<24} {:>8} {:>8} {:>8}", "arm", "r@1", "r@10", "mrr");
    println!("{}", "-".repeat(52));
    let row = |label: &str, m: (f64, f64, f64)| {
        println!("{label:<24} {:>8.3} {:>8.3} {:>8.3}", m.0, m.1, m.2);
    };
    row("none (baseline)", none);
    row("query-only", qonly);
    row("query+doc", qdoc);

    let d1 = qdoc.0 - none.0;
    let d10 = qdoc.1 - none.1;
    let dm = qdoc.2 - none.2;
    println!("\nquery+doc − baseline:  Δr@1={d1:+.3}  Δr@10={d10:+.3}  Δmrr={dm:+.3}");
    println!(
        "gate: ship iff query+doc beats baseline beyond noise on r@10/mrr \
         (then confirm no fused-hybrid regression separately)."
    );
}
