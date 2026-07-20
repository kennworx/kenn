//! Perf bench harness for the storage surface.
//!
//! Reports — does not assert — throughput and latency for the Lance +
//! redb backend.
//!
//! Benches:
//!
//! * `bulk_ingest_10k` — throughput of writing a 10k-symbol batch
//!   (~10k edges + 10k docs) through `write_batch`.
//! * `producer_to_queryable_lag` — elapsed from a write returning to
//!   the symbol becoming returnable from `Reader::fetch_symbol`.
//! * `find_symbol_tiered` — p50 / p95 (criterion handles the
//!   distribution) for an exact-name tiered lookup against a built
//!   corpus.
//! * `search_symbols_blended` — p50 / p95 for a blended query.

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use kenn_model::{
    DefRecord, EdgeProperties, EdgeRecord, FileRecord, Kind, Language, PackageRecord,
    SymbolDocsRecord, SymbolRecord,
};
use kenn_store::api::{Reader, WriteBatch};
use kenn_store::{open_writer, reader_from_writer, DbWriter, WriterOptions};
use std::path::Path;
use tempfile::TempDir;
use tokio::runtime::Runtime;

const FILE_ID: u32 = 1;
const PKG_ID: u32 = 0;

fn make_runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("rt")
}

fn corpus(
    symbols: u32,
) -> (
    FileRecord,
    PackageRecord,
    Vec<SymbolRecord>,
    Vec<DefRecord>,
    Vec<SymbolDocsRecord>,
    Vec<EdgeRecord>,
) {
    let file = FileRecord {
        id: FILE_ID,
        path: "src/lib.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 0,
    };
    let pkg = PackageRecord {
        id: PKG_ID,
        name: "kenn-bench".into(),
        version: "0.0.0".into(),
        manager: "cargo".into(),
        external: false,
    };

    let mut syms = Vec::with_capacity(symbols as usize);
    let mut defs = Vec::with_capacity(symbols as usize);
    let mut docs = Vec::with_capacity(symbols as usize);
    let mut edges = Vec::with_capacity(symbols.saturating_sub(1) as usize);

    for i in 1..=symbols {
        syms.push(SymbolRecord {
            id: i,
            pub_id: format!("rs:Sym{i:06}"),
            language: Language::Rust,
            pkg_id: PKG_ID,
            kind: Kind::Function,
            name: format!("Sym{i:06}"),
            enclosing_sym_id: 0,
            partial: false,
            nargs: 0,
            targs: 0,
            external: false,
            test: false,
        });
        defs.push(DefRecord {
            sym_id: i,
            file_id: FILE_ID,
            start_line: i,
            start_col: 0,
            end_line: i + 1,
            end_col: 0,
            body_start_line: 0,
            body_end_line: 0,
        });
        docs.push(SymbolDocsRecord {
            sym_id: i,
            sig: format!("fn Sym{i:06}()"),
            doc: format!("auto-generated bench symbol number {i}"),
        });
        if i > 1 {
            // Build a chain so list_inbound/outbound have something to
            // chase (every other entry).
            edges.push(EdgeRecord {
                src_id: i,
                target_id: i - 1,
                properties: EdgeProperties::Calls,
            });
        }
    }
    (file, pkg, syms, defs, docs, edges)
}

async fn open_writer_at(dir: &Path) -> DbWriter {
    open_writer(dir, WriterOptions::default())
        .await
        .expect("open_writer")
}

async fn ingest_corpus(writer: DbWriter, count: u32) -> DbWriter {
    let (file, pkg, syms, defs, docs, edges) = corpus(count);
    let batch = WriteBatch {
        packages: vec![pkg],
        files: vec![file],
        symbols: syms,
        symbol_docs: docs,
        file_docs: vec![],
        defs,
        edges,
    };
    writer.write_batch(&batch).await.expect("write_batch");
    writer
}

fn bench_bulk_ingest_10k(c: &mut Criterion) {
    let rt = make_runtime();
    let mut group = c.benchmark_group("bulk_ingest");
    group.sample_size(10);
    group.throughput(Throughput::Elements(10_000));
    group.bench_function("symbols_10k", |b| {
        b.iter_batched(
            || {
                let dir = TempDir::new().expect("tempdir");
                let writer = rt.block_on(async { open_writer_at(dir.path()).await });
                (dir, writer)
            },
            |(dir, writer)| {
                let writer = rt.block_on(async { ingest_corpus(writer, 10_000).await });
                drop(writer);
                drop(dir);
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn bench_producer_to_queryable_lag(c: &mut Criterion) {
    let rt = make_runtime();
    let mut group = c.benchmark_group("producer_to_queryable_lag");
    group.sample_size(10);
    group.bench_function("after_flush", |b| {
        b.iter_batched(
            || {
                let dir = TempDir::new().expect("tempdir");
                let writer = rt.block_on(async { open_writer_at(dir.path()).await });
                (dir, writer)
            },
            |(dir, writer)| {
                // Time covers: ingest a small corpus, then verify the
                // last symbol is queryable. End-to-end producer→
                // queryable lag for the configured backend.
                let writer = rt.block_on(async {
                    let w = ingest_corpus(writer, 100).await;
                    let reader = reader_from_writer(&w).await.expect("reader");
                    let row = reader
                        .fetch_symbol_by_short_id(100)
                        .await
                        .expect("fetch")
                        .expect("present");
                    assert_eq!(row.id, 100);
                    w
                });
                drop(writer);
                drop(dir);
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn bench_find_symbol_tiered(c: &mut Criterion) {
    let rt = make_runtime();
    let dir = TempDir::new().expect("tempdir");
    let writer = rt.block_on(async {
        let w = open_writer_at(dir.path()).await;
        // Hold the runtime context so SurrealdbSink's internal
        // `Handle::current().block_on(...)` resolves during ingest.
        ingest_corpus(w, 10_000).await
    });
    let reader = rt.block_on(async { reader_from_writer(&writer).await.expect("reader") });

    let mut group = c.benchmark_group("find_symbol_tiered");
    group.sample_size(20);
    group.bench_function("exact_name", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = reader
                .find_symbol_tiered("Sym001234", 10, false, false)
                .await
                .expect("find_symbol_tiered");
        });
    });
    group.finish();

    drop(reader);
    drop(writer);
    drop(dir);
}

fn bench_search_symbols_blended(c: &mut Criterion) {
    let rt = make_runtime();
    let dir = TempDir::new().expect("tempdir");
    let writer = rt.block_on(async {
        let w = open_writer_at(dir.path()).await;
        // Hold the runtime context so SurrealdbSink's internal
        // `Handle::current().block_on(...)` resolves during ingest.
        ingest_corpus(w, 10_000).await
    });
    let reader = rt.block_on(async { reader_from_writer(&writer).await.expect("reader") });

    let mut group = c.benchmark_group("search_symbols_blended");
    group.sample_size(20);
    group.bench_function("blended_query", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = reader
                .search_symbols_blended("bench symbol", None, 10, false, false)
                .await
                .expect("search_symbols_blended");
        });
    });
    group.finish();

    drop(reader);
    drop(writer);
    drop(dir);
}

criterion_group!(
    benches,
    bench_bulk_ingest_10k,
    bench_producer_to_queryable_lag,
    bench_find_symbol_tiered,
    bench_search_symbols_blended,
);
criterion_main!(benches);
