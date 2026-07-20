//! Print a short summary of the aggregated-graph tables in a snapshot.
//!
//! Used to spot-check the indexer's end-of-run aggregation pass across
//! real workspaces. Pass the snapshot directory:
//!
//!     cargo run -p kenn-store --example dump_aggregate_summary -- \
//!         /path/to/repo/.kenn/snapshots/<id>
//!
//! With no arg, looks for `.kenn/live/snapshot` under the current dir.

use std::collections::BTreeMap;
use std::path::PathBuf;

use kenn_store::api::Reader;
use kenn_store::open_reader;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot: PathBuf = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from(".kenn/live/snapshot"), PathBuf::from);

    eprintln!("opening snapshot: {}", snapshot.display());
    let reader = open_reader(&snapshot).await?;

    let nodes = reader.scan_aggregate_nodes().await?;
    let edges = reader.scan_aggregate_edges().await?;
    println!("aggregate_nodes: {}", nodes.len());
    println!("aggregate_edges: {}", edges.len());

    // Top anchors by node count.
    let mut by_anchor: BTreeMap<String, usize> = BTreeMap::new();
    for n in &nodes {
        *by_anchor.entry(n.anchor_name.clone()).or_insert(0) += 1;
    }
    let mut by_anchor_vec: Vec<(String, usize)> = by_anchor.into_iter().collect();
    by_anchor_vec.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("\ntop anchors (by node count, top 15):");
    for (name, n) in by_anchor_vec.iter().take(15) {
        println!("  {n:>6}  {name}");
    }

    // Edge breakdown by kind.
    let mut by_kind: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for e in &edges {
        let entry = by_kind
            .entry(e.kind.db_name().to_string())
            .or_insert((0, 0));
        entry.0 += 1;
        entry.1 += u64::from(e.weight);
    }
    println!("\nedges by kind:");
    for (k, (count, total_w)) in by_kind {
        println!("  {k:<14}  count={count:>6}  total_weight={total_w}");
    }

    // Top 5 heaviest edges.
    let mut sorted_edges = edges.clone();
    sorted_edges.sort_by_key(|e| std::cmp::Reverse(e.weight));
    let node_name = |sid: u32| {
        nodes.iter().find(|n| n.id == sid).map_or_else(
            || format!("#{sid}"),
            |n| format!("{}::{}", n.anchor_name, n.name),
        )
    };
    println!("\ntop 10 heaviest edges:");
    for e in sorted_edges.iter().take(10) {
        println!(
            "  {kind:<12} w={w:>5}  {a}  →  {b}",
            kind = e.kind.db_name(),
            w = e.weight,
            a = node_name(e.src_id),
            b = node_name(e.dst_id),
        );
    }

    Ok(())
}
