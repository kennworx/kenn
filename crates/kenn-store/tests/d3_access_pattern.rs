//! Task 9.5 — the D3 access-pattern regression guard.
//!
//! Design D3 forbids a per-item Lance query loop: traversal SHALL
//! bulk-scan a dataset once into an in-memory structure (the CSR edge
//! adjacency), and hydration (ids → rows) SHALL issue one batched
//! `take()`. A reintroduced per-item loop costs the ~175 µs Lance
//! query-planning floor × N — seconds-slow.
//!
//! This guard builds a real code graph — 40 000 symbols, 120 000 edges
//! — and times the two D3-governed reader surfaces:
//!
//! - **CSR build.** `open_reader` bulk-scans the `edges` dataset once
//!   into the in-memory adjacency (and the `defs` dataset into the
//!   resident def map). A regression that scanned per vertex would cost
//!   ≈ 40 000 × 175 µs ≈ 7 s.
//! - **Traversal + hydration.** `list_outbound` on a hub symbol walks
//!   the in-RAM CSR and resolves ~40 000 neighbour ids to `SymbolRow`s
//!   with one batched `take()`. A regression to one `take()` per id
//!   would likewise cost ≈ 7 s.
//!
//! Either is an order of magnitude over `THRESHOLD`; the correct path —
//! bulk scans + in-RAM walks — runs in a few hundred milliseconds even
//! in a debug build.

use std::time::{Duration, Instant};

use kenn_model::{DefRecord, EdgeProperties, EdgeRecord, FileRecord, Kind, Language, SymbolRecord};
use kenn_store::api::{Reader, WriteBatch};
use kenn_store::{open_reader, open_writer, WriterOptions};
use tempfile::TempDir;

const SYMBOLS: u32 = 40_000;
const EDGES: usize = 120_000;

/// The hub symbol — the lexicographic edge order makes the lowest id
/// the densest source, so one `list_outbound` call on it hydrates the
/// largest possible id set.
const HUB: u32 = 1;

/// The correct path — bulk scans + in-RAM walks — runs in a few hundred
/// milliseconds in a debug build. The cheapest regression D3 forbids (a
/// per-vertex scan or per-id `take()`, ≈ 40 000 × 175 µs ≈ 7 s) is an
/// order of magnitude slower, so a 2 s bound trips it decisively (~3.5×)
/// while leaving the correct path ~4× headroom against scan jitter.
const THRESHOLD: Duration = Duration::from_secs(2);

fn symbol(id: u32) -> SymbolRecord {
    SymbolRecord {
        id,
        pub_id: format!("rs:Sym{id}"),
        language: Language::Rust,
        pkg_id: 0,
        kind: Kind::Function,
        name: format!("Sym{id}"),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    }
}

fn def(id: u32) -> DefRecord {
    DefRecord {
        sym_id: id,
        file_id: 1,
        start_line: id,
        start_col: 0,
        end_line: id,
        end_col: 1,
        body_start_line: 0,
        body_end_line: 0,
    }
}

/// A deterministic representative code graph: every symbol is a vertex;
/// edges are the first `EDGES` distinct `(source, target)` pairs in
/// lexicographic order, so the low-numbered symbols — `HUB` first — are
/// dense hubs and one `list_outbound` call hydrates a ~`SYMBOLS`-sized
/// id set.
fn code_graph() -> WriteBatch {
    let mut batch = WriteBatch::default();
    batch.files.push(FileRecord {
        id: 1,
        path: "src/lib.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 0,
    });
    batch.symbols = (1..=SYMBOLS).map(symbol).collect();
    batch.defs = (1..=SYMBOLS).map(def).collect();
    'outer: for src in 1..=SYMBOLS {
        for tgt in (src + 1)..=SYMBOLS {
            batch.edges.push(EdgeRecord {
                src_id: src,
                target_id: tgt,
                properties: EdgeProperties::Calls,
            });
            if batch.edges.len() == EDGES {
                break 'outer;
            }
        }
    }
    batch
}

#[tokio::test(flavor = "multi_thread")]
async fn d3_reader_surface_has_no_per_item_loop() {
    let dir = TempDir::new().unwrap();
    let snapshot = dir.path().join("snapshot");

    // Build the corpus — untimed.
    let batch = code_graph();
    let writer = open_writer(&snapshot, WriterOptions::default())
        .await
        .expect("open_writer");
    writer.write_batch(&batch).await.expect("write_batch");
    drop(writer);

    // The timed region: the two D3-governed reader surfaces.
    let start = Instant::now();

    // CSR build — `open_reader` bulk-scans the `edges` and `defs`
    // datasets once into the in-memory adjacency / resident maps.
    let reader = open_reader(&snapshot).await.expect("open_reader");

    // Traversal + batched hydration — walk the hub's CSR adjacency and
    // resolve every neighbour id to a `SymbolRow` in one `take()`.
    let (neighbours, total) = reader
        .list_outbound(HUB, "calls", SYMBOLS, None, true, true)
        .await
        .expect("list_outbound");

    let elapsed = start.elapsed();

    assert_eq!(
        neighbours.len(),
        (SYMBOLS - 1) as usize,
        "the hub calls every other symbol"
    );
    assert_eq!(total, u64::from(SYMBOLS - 1), "reported total matches");
    assert!(
        elapsed < THRESHOLD,
        "D3 reader surface took {elapsed:?} (bound {THRESHOLD:?}) — a \
         per-item Lance query loop likely regressed (design D3)"
    );
    eprintln!("D3 reader surface ({SYMBOLS} symbols, {EDGES} edges): {elapsed:?}");
}
