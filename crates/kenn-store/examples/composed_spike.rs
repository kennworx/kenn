//! Eval rig: validate the FULL COMPOSED blended fusion (the acceptance-gate for
//! the `rrf-identifier-fusion` change). Earlier harnesses measured each arm in
//! isolation; this runs the whole stack as one ranker, with vs without the
//! word-split name-token arm, to answer:
//!   (a) does the name-token arm hurt exact-identifier recall (G1)?
//!   (b) does it help multi-word identifier recall (G1b) — esp. `snake_case`?
//!   (c) does it regress conceptual recall (G2) by firing on prose content words?
//! and (caveat #2) does a doc-only vector index beat the stored `sig+doc` one.
//!
//! Variants (both RRF K=60 + exact-name bonus):
//!   V1i  = RRF{ `name_lower`, `sig(name_fts)`, `doc(doc_fts)`, vector }
//!   V1t  = V1i + word-split name-token arm
//!
//! Findings (kenn corpus): V1t lifts G1b multi-word (+61% MRR) but wrecks G2
//! conceptual (−46%) — so the name-token arm belongs in `find_symbol_tiered`,
//! not blended. doc-only vector lifts G2 (+19%). See the change's design.md.
//!
//! Usage: `cargo run -p kenn-store --example composed_spike -- [snapshot-dir]`

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use kenn_embed::{EmbedKind, EmbeddingProducer, LlamaEmbedder};
use rusqlite::{Connection, OpenFlags};

const RRF_K: f64 = 60.0;
const W_NAME_LOWER: f64 = 1.0;
const W_SIG: f64 = 1.0;
const W_DOC: f64 = 0.7;
const W_VEC: f64 = 1.0;
const EXACT_BONUS: f64 = 1.0;
const DEPTH: usize = 20;

/// Name-token arm weight, overridable via `W_NT` env for the sweep. Default 1.0.
fn w_name_token() -> f64 {
    std::env::var("W_NT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0)
}

// Centralized narrowing casts — SQLite row ids and pool/limit/metric counts are
// small and non-negative, so the cast lints are intentional in these helpers.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "SQLite ids are small non-negative integers"
)]
fn u32c(v: i64) -> u32 {
    v as u32
}
#[expect(clippy::cast_possible_wrap, reason = "pool/limit counts are small")]
fn i64c(v: usize) -> i64 {
    v as i64
}
#[expect(
    clippy::cast_precision_loss,
    reason = "metric counts are well under 2^52"
)]
fn f64c(v: usize) -> f64 {
    v as f64
}

fn register_vec() {
    #[expect(
        unsafe_code,
        clippy::missing_transmute_annotations,
        clippy::multiple_unsafe_ops_per_block,
        reason = "FFI registration copied from kenn-store's register.rs"
    )]
    // SAFETY: `sqlite3_vec_init` is a valid C-ABI extension entry point of the
    // shape `sqlite3_auto_extension` expects; registered before any vec0 open.
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }
}

#[expect(clippy::panic, reason = "eval rig: fail fast on a bad snapshot path")]
fn open_ro(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap_or_else(|e| panic!("open {} read-only: {e}", path.display()))
}

fn split_name(name: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if c == '_' {
            out.push(' ');
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower {
            out.push(' ');
        }
        out.push(c);
        prev_lower = c.is_lowercase() || c.is_numeric();
    }
    out.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn is_multiword(name: &str) -> bool {
    name.contains('_')
        || name
            .chars()
            .zip(name.chars().skip(1))
            .any(|(a, b)| a.is_lowercase() && b.is_uppercase())
}

// ---- arms: each returns symbol ids best-first ----

fn name_fts_arm(k: &Connection, query: &str, pool: usize) -> Vec<u32> {
    let cleaned: String = query.chars().filter(|c| c.is_alphanumeric()).collect();
    if cleaned.len() < 3 {
        return Vec::new();
    }
    let mut s = k
        .prepare(
            "SELECT k.id FROM name_fts JOIN knowledge k ON k.rowid=name_fts.rowid \
             WHERE name_fts MATCH ?1 ORDER BY bm25(name_fts) LIMIT ?2",
        )
        .expect("p");
    s.query_map(
        rusqlite::params![format!("\"{cleaned}\""), i64c(pool)],
        |r| Ok(u32c(r.get(0)?)),
    )
    .expect("q")
    .map(|r| r.expect("r"))
    .collect()
}

fn doc_arm(k: &Connection, query: &str, pool: usize, holdout: Option<u32>) -> Vec<u32> {
    let q = query.replace('"', "");
    if q.trim().is_empty() {
        return Vec::new();
    }
    let mut s = k
        .prepare(
            "SELECT k.id, k.pub_id FROM doc_fts JOIN knowledge k ON k.rowid=doc_fts.rowid \
             WHERE doc_fts MATCH ?1 ORDER BY bm25(doc_fts) LIMIT ?2",
        )
        .expect("p");
    s.query_map(rusqlite::params![format!("\"{q}\""), i64c(pool)], |r| {
        Ok((u32c(r.get(0)?), r.get::<_, String>(1)?))
    })
    .expect("q")
    .filter_map(|r| {
        let (id, pub_id) = r.expect("r");
        (!pub_id.is_empty() && Some(id) != holdout).then_some(id)
    })
    .collect()
}

fn vec_arm(k: &Connection, qv: &[f32], pool: usize) -> Vec<u32> {
    let bytes: Vec<u8> = qv.iter().flat_map(|f| f.to_le_bytes()).collect();
    let mut s = k
        .prepare(
            "SELECT kr.id FROM vec_knowledge JOIN knowledge kr ON kr.rowid=vec_knowledge.rowid \
             WHERE vec_knowledge.embedding MATCH ?1 AND vec_knowledge.k=?2 ORDER BY distance",
        )
        .expect("p");
    let mut seen = HashSet::new();
    s.query_map(rusqlite::params![bytes, i64c(pool)], |r| {
        Ok(u32c(r.get(0)?))
    })
    .expect("q")
    .filter_map(|r| {
        let id = r.expect("r");
        seen.insert(id).then_some(id)
    })
    .collect()
}

fn name_lower_arm(g: &Connection, query: &str, pool: usize) -> Vec<u32> {
    let ql = query.to_ascii_lowercase();
    if ql.trim().is_empty() {
        return Vec::new();
    }
    let esc = ql
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (pred, pat) in [
        ("name_lower = ?1".to_string(), ql.clone()),
        (
            "name_lower LIKE ?1 ESCAPE '\\'".to_string(),
            format!("{esc}%"),
        ),
        (
            "name_lower LIKE ?1 ESCAPE '\\'".to_string(),
            format!("%{esc}%"),
        ),
    ] {
        let sql = format!(
            "SELECT id FROM symbols WHERE {pred} AND external=0 AND test=0 \
             ORDER BY length(name), id LIMIT ?2"
        );
        let mut s = g.prepare(&sql).expect("p");
        let rows = s
            .query_map(rusqlite::params![pat, i64c(pool)], |r| Ok(u32c(r.get(0)?)))
            .expect("q");
        for r in rows {
            let id = r.expect("r");
            if seen.insert(id) {
                out.push(id);
            }
        }
    }
    out
}

/// Word-split name-token arm: in-memory `unicode61` FTS5 over `split_name`s.
struct NameTokenIndex {
    mem: Connection,
    ids: Vec<u32>,
}
impl NameTokenIndex {
    fn build(g: &Connection) -> Self {
        let mut s = g
            .prepare("SELECT id, name FROM symbols WHERE external=0 AND test=0")
            .expect("p");
        let rows: Vec<(u32, String)> = s
            .query_map([], |r| Ok((u32c(r.get(0)?), r.get::<_, String>(1)?)))
            .expect("q")
            .map(|r| r.expect("r"))
            .collect();
        let mem = Connection::open_in_memory().expect("mem");
        mem.execute_batch("CREATE VIRTUAL TABLE n USING fts5(toks, tokenize='unicode61');")
            .expect("create");
        let mut ids = Vec::with_capacity(rows.len());
        {
            let mut ins = mem
                .prepare("INSERT INTO n(rowid,toks) VALUES(?1,?2)")
                .expect("pi");
            for (i, (id, name)) in rows.iter().enumerate() {
                ins.execute(rusqlite::params![i64c(i + 1), split_name(name)])
                    .expect("ins");
                ids.push(*id);
            }
        }
        Self { mem, ids }
    }
    fn search(&self, query: &str, pool: usize) -> Vec<u32> {
        let expr = split_name(query)
            .split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(char::is_ascii_alphanumeric)
                    .collect::<String>()
            })
            .filter(|w| !w.is_empty())
            .map(|w| format!("\"{w}\""))
            .collect::<Vec<_>>()
            .join(" OR ");
        if expr.is_empty() {
            return Vec::new();
        }
        let mut s = self
            .mem
            .prepare("SELECT rowid FROM n WHERE n MATCH ?1 ORDER BY bm25(n) LIMIT ?2")
            .expect("p");
        s.query_map(rusqlite::params![expr, i64c(pool)], |r| r.get::<_, i64>(0))
            .expect("q")
            .map(|r| {
                let idx = usize::try_from(r.expect("r")).expect("rowid") - 1;
                *self.ids.get(idx).expect("rowid in range")
            })
            .collect()
    }
}

fn rrf_into(scores: &mut HashMap<u32, f64>, arm: &[u32], w: f64) {
    for (i, id) in arm.iter().enumerate() {
        *scores.entry(*id).or_default() += w / (RRF_K + f64c(i + 1));
    }
}

struct Meta {
    name_lower: String,
    name_len: usize,
}
fn fetch_meta(g: &Connection, ids: &[u32]) -> HashMap<u32, Meta> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return out;
    }
    let ph = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT id, name FROM symbols WHERE id IN ({ph})");
    let mut s = g.prepare(&sql).expect("p");
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|i| i as &dyn rusqlite::ToSql).collect();
    let rows = s
        .query_map(params.as_slice(), |r| {
            let name: String = r.get(1)?;
            Ok((
                u32c(r.get(0)?),
                Meta {
                    name_lower: name.to_ascii_lowercase(),
                    name_len: name.len(),
                },
            ))
        })
        .expect("q");
    for r in rows {
        let (id, m) = r.expect("r");
        out.insert(id, m);
    }
    out
}

#[expect(clippy::too_many_arguments, reason = "eval-rig arm wiring")]
fn rank(
    use_token: bool,
    k: &Connection,
    g: &Connection,
    nt: &NameTokenIndex,
    query: &str,
    qv: Option<&[f32]>,
    holdout: Option<u32>,
    doconly: Option<&HashMap<u32, Vec<f32>>>,
) -> Vec<u32> {
    let pool = (DEPTH * 8).max(64);
    let nl = name_lower_arm(g, query, pool);
    let sig = name_fts_arm(k, query, pool);
    let doc = doc_arm(k, query, pool, holdout);
    let vec = match (qv, doconly) {
        (Some(v), Some(map)) => vec_arm_doconly(map, v, pool),
        (Some(v), None) => vec_arm(k, v, pool),
        (None, _) => Vec::new(),
    };
    let tok = if use_token {
        nt.search(query, pool)
    } else {
        Vec::new()
    };

    let ids: Vec<u32> = nl
        .iter()
        .chain(&sig)
        .chain(&doc)
        .chain(&vec)
        .chain(&tok)
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let meta = fetch_meta(g, &ids);
    let ql = query.to_ascii_lowercase();

    let mut scores: HashMap<u32, f64> = HashMap::new();
    rrf_into(&mut scores, &nl, W_NAME_LOWER);
    rrf_into(&mut scores, &sig, W_SIG);
    rrf_into(&mut scores, &doc, W_DOC);
    rrf_into(&mut scores, &vec, W_VEC);
    if use_token {
        rrf_into(&mut scores, &tok, w_name_token());
    }
    for (id, m) in &meta {
        if m.name_lower == ql {
            *scores.entry(*id).or_default() += EXACT_BONUS;
        }
    }

    let mut hits: Vec<(u32, f64, usize)> = scores
        .into_iter()
        .filter_map(|(id, sc)| meta.get(&id).map(|m| (id, sc, m.name_len)))
        .collect();
    hits.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.0.cmp(&b.0))
    });
    hits.truncate(DEPTH);
    hits.into_iter().map(|(id, _, _)| id).collect()
}

fn embed_one(e: &LlamaEmbedder, rt: &tokio::runtime::Runtime, q: &str) -> Option<Vec<f32>> {
    rt.block_on(e.embed(&[q], EmbedKind::Document))
        .ok()
        .and_then(|mut v| v.drain(..).next())
}

fn embed_all(e: &LlamaEmbedder, rt: &tokio::runtime::Runtime, texts: &[String]) -> Vec<Vec<f32>> {
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut out = Vec::with_capacity(texts.len());
    for chunk in refs.chunks(32) {
        out.extend(
            rt.block_on(e.embed(chunk, EmbedKind::Document))
                .expect("embed batch"),
        );
    }
    out
}

/// Strip XML/HTML tags (C# `<summary>` docs) and collapse whitespace.
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

/// Doc-only vector index (Change C): embed each documented symbol's cleaned doc
/// prose (NOT `sig+doc`), keyed by symbol id. Caveat #2 test: does swapping the
/// fusion's vector arm to this hurt conceptual recall vs the stored `sig+doc`?
fn build_doconly(
    g: &Connection,
    e: &LlamaEmbedder,
    rt: &tokio::runtime::Runtime,
) -> HashMap<u32, Vec<f32>> {
    let mut s = g
        .prepare(
            "SELECT d.sym_id, d.doc FROM symbol_docs d JOIN symbols s ON s.id=d.sym_id \
             WHERE s.external=0 AND s.test=0 AND length(d.doc)>=60 ORDER BY d.sym_id",
        )
        .expect("p");
    let rows: Vec<(u32, String)> = s
        .query_map([], |r| {
            Ok((u32c(r.get(0)?), clean_doc(&r.get::<_, String>(1)?)))
        })
        .expect("q")
        .map(|r| r.expect("r"))
        .filter(|(_, d)| d.len() >= 30)
        .collect();
    let texts: Vec<String> = rows.iter().map(|(_, d)| d.clone()).collect();
    let vecs = embed_all(e, rt, &texts);
    rows.iter().map(|(id, _)| *id).zip(vecs).collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum() // L2-normalized vectors → dot = cosine
}

fn vec_arm_doconly(map: &HashMap<u32, Vec<f32>>, qv: &[f32], pool: usize) -> Vec<u32> {
    let mut scored: Vec<(u32, f32)> = map.iter().map(|(id, v)| (*id, cosine(qv, v))).collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.into_iter().take(pool).map(|(id, _)| id).collect()
}

// ---- gold sets ----

fn unique_names(g: &Connection, multiword_only: bool, n: usize) -> Vec<(String, u32)> {
    let mut s = g
        .prepare(
            "SELECT name, id FROM symbols WHERE external=0 AND test=0 AND length(name)>=3 \
             AND name IN (SELECT name FROM symbols GROUP BY name HAVING count(*)=1) ORDER BY id",
        )
        .expect("p");
    let all: Vec<(String, u32)> = s
        .query_map([], |r| Ok((r.get::<_, String>(0)?, u32c(r.get(1)?))))
        .expect("q")
        .map(|r| r.expect("r"))
        .filter(|(name, _)| !multiword_only || is_multiword(name))
        .collect();
    if all.is_empty() {
        return Vec::new();
    }
    let stride = (all.len() / n).max(1);
    all.into_iter().step_by(stride).take(n).collect()
}

fn gen_g2(g: &Connection, n: usize) -> Vec<(String, u32)> {
    let mut s = g
        .prepare(
            "SELECT d.sym_id, d.doc FROM symbol_docs d JOIN symbols s ON s.id=d.sym_id \
             WHERE s.external=0 AND s.test=0 AND length(d.doc)>=60 ORDER BY d.sym_id",
        )
        .expect("p");
    let all: Vec<(u32, String)> = s
        .query_map([], |r| Ok((u32c(r.get(0)?), r.get::<_, String>(1)?)))
        .expect("q")
        .map(|r| r.expect("r"))
        .collect();
    if all.is_empty() {
        return Vec::new();
    }
    let stride = (all.len() / n).max(1);
    all.into_iter()
        .step_by(stride)
        .take(n)
        .map(|(id, doc)| {
            let line = doc
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("");
            (
                line.split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(160)
                    .collect(),
                id,
            )
        })
        .collect()
}

#[expect(clippy::too_many_arguments, reason = "eval-rig wiring")]
fn eval(
    use_token: bool,
    k: &Connection,
    g: &Connection,
    nt: &NameTokenIndex,
    e: &LlamaEmbedder,
    rt: &tokio::runtime::Runtime,
    gold: &[(String, u32)],
    transform_query: impl Fn(&str, u32) -> String,
    holdout_self: bool,
    use_vec: bool,
    doconly: Option<&HashMap<u32, Vec<f32>>>,
) -> (f64, f64, f64) {
    let (mut r1, mut r10, mut mrr) = (0usize, 0usize, 0.0f64);
    for (raw, id) in gold {
        let q = transform_query(raw, *id);
        let qv = if use_vec { embed_one(e, rt, &q) } else { None };
        let holdout = holdout_self.then_some(*id);
        let ranked = rank(use_token, k, g, nt, &q, qv.as_deref(), holdout, doconly);
        if let Some(p) = ranked.iter().position(|x| x == id) {
            if p < 1 {
                r1 += 1;
            }
            if p < 10 {
                r10 += 1;
            }
            mrr += 1.0 / f64c(p + 1);
        }
    }
    let nn = f64c(gold.len().max(1));
    (f64c(r1) / nn, f64c(r10) / nn, mrr / nn)
}

fn main() {
    register_vec();
    let snap = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| ".kenn/local/live".into()),
    );
    let snap = std::fs::canonicalize(&snap).expect("canon");
    eprintln!("snapshot = {}", snap.display());
    let k = open_ro(&snap.join("vector.db"));
    let g = open_ro(&snap.join("code.db"));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("rt");
    eprintln!("loading embedder + building name-token index...");
    let e = LlamaEmbedder::load("embeddinggemma-300M".to_owned()).expect("embed");
    let nt = NameTokenIndex::build(&g);

    let g1 = unique_names(&g, false, 200);
    let g1b = unique_names(&g, true, 200);
    let g2 = gen_g2(&g, 200);
    eprintln!("G1={} G1b={} G2={}", g1.len(), g1b.len(), g2.len());

    let id = |s: &str, _: u32| s.to_string();
    let words = |s: &str, _: u32| split_name(s);

    println!(
        "\n{:<30} {:>8} {:>8} {:>8}",
        "gold / variant", "r@1", "r@10", "mrr"
    );
    println!("{}", "-".repeat(58));
    let row = |label: &str, m: (f64, f64, f64)| {
        println!("{:<30} {:>8.3} {:>8.3} {:>8.3}", label, m.0, m.1, m.2);
    };

    // Caveat #1 (composed config): name-token arm helps G1b, wrecks G2.
    row(
        "G1b mword  V1i",
        eval(false, &k, &g, &nt, &e, &rt, &g1b, words, false, true, None),
    );
    row(
        "G1b mword  V1t (+token)",
        eval(true, &k, &g, &nt, &e, &rt, &g1b, words, false, true, None),
    );

    // Caveat #2 (doc-only embeddings inside the fusion): build a doc-only vector
    // index and swap it in for the stored sig+doc vectors. V1i only (the
    // validated blended config; name-token excluded per caveat #1).
    eprintln!("building doc-only vector index...");
    let doconly = build_doconly(&g, &e, &rt);
    eprintln!("doc-only index: {} documented symbols", doconly.len());
    let dc = Some(&doconly);

    row(
        "G1 exact   V1i  sig+doc",
        eval(false, &k, &g, &nt, &e, &rt, &g1, id, false, true, None),
    );
    row(
        "G1 exact   V1i  doc-only",
        eval(false, &k, &g, &nt, &e, &rt, &g1, id, false, true, dc),
    );
    row(
        "G2 concept V1i  sig+doc",
        eval(false, &k, &g, &nt, &e, &rt, &g2, id, true, true, None),
    );
    row(
        "G2 concept V1i  doc-only",
        eval(false, &k, &g, &nt, &e, &rt, &g2, id, true, true, dc),
    );

    println!(
        "\nCaveat #2: doc-only vector inside the fusion should be >= sig+doc on G2\n\
         (conceptual) and tie on G1 (exact-id is name_lower-driven, vector minor)."
    );
}
