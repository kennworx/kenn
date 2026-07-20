//! Diagnostic: count symbols by (external, test) buckets in a snapshot.
//! Verifies that ingest-time test detection populates `SymbolRecord.test`.
//!
//! Usage: `cargo run --release -p kenn-store --example dump_test_summary -- <snapshot_dir>`

use std::collections::BTreeMap;
use std::path::PathBuf;

use kenn_store::api::Reader;
use kenn_store::open_reader;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arg = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: dump_test_summary <snapshot_dir>"))?;
    let snapshot = PathBuf::from(arg);

    let reader = open_reader(&snapshot).await?;
    let symbols = reader.scan_symbols().await?;

    let mut total = 0u64;
    let mut by_bucket: BTreeMap<(bool, bool), u64> = BTreeMap::new();
    let mut by_kind_test: BTreeMap<String, u64> = BTreeMap::new();
    let mut sample_user_tests: Vec<String> = Vec::new();

    for s in &symbols {
        total += 1;
        *by_bucket.entry((s.external, s.test)).or_insert(0) += 1;
        if s.test && !s.external {
            *by_kind_test.entry(s.kind.clone()).or_insert(0) += 1;
            if sample_user_tests.len() < 10 {
                sample_user_tests.push(format!("  {} ({}) [{}]", s.name, s.kind, s.pub_id));
            }
        }
    }

    println!("snapshot: {}", snapshot.display());
    println!("total symbols: {total}");
    println!();
    println!("buckets (external, test) → count:");
    for ((ext, test), n) in &by_bucket {
        println!("  external={ext:<5} test={test:<5}  {n}");
    }
    println!();
    println!("user-test symbols by kind:");
    let mut kind_sorted: Vec<_> = by_kind_test.iter().collect();
    kind_sorted.sort_by(|a, b| b.1.cmp(a.1));
    for (k, n) in kind_sorted {
        println!("  {k:<14} {n}");
    }
    if !sample_user_tests.is_empty() {
        println!();
        println!("sample user-test symbols:");
        for line in &sample_user_tests {
            println!("{line}");
        }
    }
    Ok(())
}
