//! Inspect a snapshot's code-graph Lance datasets — per-table row counts.
//!
//! Usage:
//!
//!     cargo run --release -p kenn-store --example inspect_store -- \
//!         /path/to/.kenn/local/live

use std::path::PathBuf;

use kenn_store::api::Reader;
use kenn_store::open_reader;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let snapshot = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("usage: inspect_store <snapshot-dir>"))?,
    );
    // Resolve the symlink if present (e.g. `.kenn/local/live`).
    let snapshot = std::fs::canonicalize(&snapshot)?;
    eprintln!("inspecting {}", snapshot.display());

    let reader = open_reader(&snapshot).await?;
    for table in [
        "files",
        "packages",
        "symbols",
        "symbol_docs",
        "defs",
        "edges",
    ] {
        let count = reader.count_table(table).await?;
        println!("{table:<14} {count:>10}");
    }
    Ok(())
}
