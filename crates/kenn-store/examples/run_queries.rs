//! Open a published snapshot, run a battery of `find_symbol_tiered`
//! and `search_symbols_blended` queries, dump the result rows as JSON
//! Lines (one per query × backend × method).
//!
//! Build under each feature, point at each backend's snapshot, diff
//! the JSONL outputs to verify the two backends agree on what an agent
//! would see.

use std::path::PathBuf;

use kenn_store::api::Reader;
use kenn_store::{open_reader, ACTIVE_BACKEND};
use serde::Serialize;

#[derive(Serialize)]
struct ResultRow {
    backend: &'static str,
    method: &'static str,
    query: String,
    rank: usize,
    short_id: u32,
    name: String,
    kind: String,
    pub_id: String,
    language: String,
    score: f64,
    match_kind: Option<String>,
}

const QUERIES: &[&str] = &[
    // Likely-exact identifier names (sample workspace domain)
    "OrderHandler",
    "WalletService",
    "AccountController",
    "CancelOrderHandler",
    // Partials / camel sub-tokens
    "Order",
    "Wallet",
    "Cancel",
    "Handler",
    // Acronyms + Pascal
    "KycService",
    "ApiClient",
    // Single-token doc match candidates
    "signup",
    "cancellation",
    "deposit",
    // Multi-token
    "cancel order",
    "wallet service",
    // Common verbs the docs reference
    "create",
    "update",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: run_queries <snapshot-dir>")?,
    );
    let snapshot = std::fs::canonicalize(&snapshot)?;
    eprintln!("backend={ACTIVE_BACKEND} snapshot={}", snapshot.display());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    let reader = rt.block_on(open_reader(&snapshot))?;

    for q in QUERIES {
        // find_symbol_tiered top-10
        let tiered = rt.block_on(async {
            reader
                .find_symbol_tiered(q, 10, false, false)
                .await
                .expect("find_symbol_tiered")
        });
        for (rank, r) in tiered.iter().enumerate() {
            let row = ResultRow {
                backend: ACTIVE_BACKEND,
                method: "find_symbol_tiered",
                query: (*q).to_string(),
                rank,
                short_id: r.symbol.id,
                name: r.symbol.name.clone(),
                kind: r.symbol.kind.clone(),
                pub_id: r.symbol.pub_id.clone(),
                language: r.symbol.language.clone(),
                score: 0.0,
                match_kind: Some(format!("{:?}", r.match_kind).to_lowercase()),
            };
            println!("{}", serde_json::to_string(&row)?);
        }

        // search_symbols_blended top-10
        let blended = rt.block_on(async {
            reader
                .search_symbols_blended(q, None, 10, false, false)
                .await
                .expect("search_symbols_blended")
        });
        for (rank, r) in blended.iter().enumerate() {
            let row = ResultRow {
                backend: ACTIVE_BACKEND,
                method: "search_symbols_blended",
                query: (*q).to_string(),
                rank,
                short_id: r.symbol.id,
                name: r.symbol.name.clone(),
                kind: r.symbol.kind.clone(),
                pub_id: r.symbol.pub_id.clone(),
                language: r.symbol.language.clone(),
                score: r.score,
                match_kind: None,
            };
            println!("{}", serde_json::to_string(&row)?);
        }
    }

    Ok(())
}
