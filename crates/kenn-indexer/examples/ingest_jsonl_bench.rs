//! Standalone JSONL → kenn-store ingest bench.
//!
//! Bypasses kenn-dotnet entirely. Reads a captured `.jsonl` file (the
//! producer's stdout, preserved via `KENN_KEEP_JSONL=1`) and feeds it
//! through `ingest_jsonl_into_sink` → a `BatchSink` against a fresh
//! kenn-store writer in a tempdir. Reports total wall time, throughput,
//! and the snapshot's on-disk size.
//!
//! Usage:
//!
//!     cargo run --release -p kenn-indexer --example ingest_jsonl_bench -- \
//!         --jsonl /path/to/captured.jsonl

use std::io::BufReader;
use std::path::PathBuf;
use std::time::Instant;

use kenn_indexer::canonicalize::Workspace;
use kenn_indexer::sink::BatchSink;
use kenn_indexer::transform::IdRegistry;
use kenn_indexer::transform_jsonl::{flush_registry_stubs, ingest_jsonl_into_sink};
use kenn_model::Language;
use kenn_store::api::DEFAULT_BATCH_SIZE;
use kenn_store::{open_writer, WriterOptions, ACTIVE_BACKEND};
use tempfile::TempDir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut jsonl: Option<PathBuf> = None;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--jsonl" => {
                jsonl = args.next().map(PathBuf::from);
            }
            other => return Err(format!("unknown flag: {other}").into()),
        }
    }
    let jsonl = jsonl.ok_or("--jsonl <path> required")?;
    let jsonl_size = std::fs::metadata(&jsonl)?.len();

    eprintln!(
        "── ingest bench: backend={ACTIVE_BACKEND} jsonl={} ({} MiB) ──",
        jsonl.display(),
        jsonl_size / (1024 * 1024)
    );

    let tmp = TempDir::new()?;
    let workspace_root = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace_root)?;
    let snapshot_dir = tmp.path().join("snapshot");
    std::fs::create_dir_all(&snapshot_dir)?;

    let workspace = Workspace::new(&workspace_root, &[])?;

    // The async runtime drives the Lance store. The main thread carries
    // no runtime context, so `BatchSink` drives appends via the handle.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()?;

    let t_open = Instant::now();
    let writer = rt.block_on(open_writer(&snapshot_dir, WriterOptions::default()))?;
    eprintln!("open_writer: {:.3} s", t_open.elapsed().as_secs_f64());

    // Each ingester owns its `BatchSink` and appends directly — no
    // channel, no DB-writer thread (retire-redb D9).
    let mut sink = BatchSink::new(writer.clone(), rt.handle().clone(), DEFAULT_BATCH_SIZE);
    let mut registry = IdRegistry::new(Language::Csharp);

    let f = std::fs::File::open(&jsonl)?;
    let mut reader = BufReader::with_capacity(1024 * 1024, f);
    let t_ingest = Instant::now();
    let stats = ingest_jsonl_into_sink(&mut reader, &workspace, &mut registry, &mut sink)?;
    flush_registry_stubs(&mut registry, &mut sink)?;
    sink.finish()?;
    let ingest_ms = t_ingest.elapsed();
    eprintln!(
        "ingest_jsonl_into_sink: {:.3} s — files={} packages={} symbols={} defs={} edges={} errors={}",
        ingest_ms.as_secs_f64(),
        stats.files,
        stats.packages,
        stats.symbols,
        stats.defs,
        stats.edges,
        stats.errors,
    );

    let t_end = Instant::now();
    rt.block_on(writer.finalize())?;
    eprintln!("finalize (commit): {:.3} s", t_end.elapsed().as_secs_f64());

    drop(writer);

    let snapshot_size = dir_size(&snapshot_dir)?;
    let total = t_open.elapsed();
    #[expect(
        clippy::cast_precision_loss,
        reason = "bench counts never approach 2^52; f64 throughput is display-only"
    )]
    let throughput = (stats.files + stats.symbols + stats.edges) as f64 / total.as_secs_f64();

    eprintln!("── totals ──");
    eprintln!("backend:           {ACTIVE_BACKEND}");
    eprintln!("wall (open→drop):  {:.3} s", total.as_secs_f64());
    eprintln!("snapshot size:     {} MiB", snapshot_size / (1024 * 1024));
    eprintln!("throughput:        {throughput:.0} files+symbols+edges/s");

    // Machine-readable single line for awk/grep at end.
    println!(
        "{{\"backend\":\"{ACTIVE_BACKEND}\",\"wall_s\":{:.3},\"ingest_s\":{:.3},\"end_run_s\":{:.3},\"snapshot_bytes\":{},\"files\":{},\"packages\":{},\"symbols\":{},\"defs\":{},\"edges\":{},\"errors\":{}}}",
        total.as_secs_f64(),
        ingest_ms.as_secs_f64(),
        t_end.elapsed().as_secs_f64(),
        snapshot_size,
        stats.files,
        stats.packages,
        stats.symbols,
        stats.defs,
        stats.edges,
        stats.errors,
    );

    Ok(())
}

fn dir_size(p: &std::path::Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in walkdir(p)? {
        let m = std::fs::metadata(&entry)?;
        if m.is_file() {
            total = total.saturating_add(m.len());
        }
    }
    Ok(total)
}

fn walkdir(p: &std::path::Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let m = std::fs::metadata(&d)?;
        if m.is_file() {
            out.push(d);
            continue;
        }
        if m.is_dir() {
            for entry in std::fs::read_dir(&d)? {
                let e = entry?;
                stack.push(e.path());
            }
        }
    }
    Ok(out)
}
