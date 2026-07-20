## Why

`SurrealdbSink::write_batch` formerly built each batch as one big
`SurrealQL` string with all record data inlined as literal arrays
(`INSERT INTO symbols [{...}, {...}, ...]`) plus one `RELATE
source->kind->target` statement per edge. On the app fixture
(728k frames, 96 MB stream) this serialized into 543k separate
`RELATE` statements per run, all parsed by `SurrealDB`'s `SurrealQL`
parser before any storage work happened.

Profiling showed the parser dominated end-to-end ingest cost. With
`SurrealDB`'s in-memory backend the wall went *up* 70% — confirming
`RocksDB` was already cheap, the bottleneck lived inside the query
execution layer (parser, planner, schema validation, index updates).

## What Changes

- **BREAKING (internal)**: `SurrealdbSink::write_batch` now emits
  parameter-bound queries instead of inline-array `SurrealQL`:
  - `INSERT INTO <table> $<table>` per non-empty record table
    (files, packages, symbols, symbol_docs, defs)
  - `INSERT RELATION INTO <kind> $b_<kind>` per non-empty edge kind
    (replacing per-edge `RELATE` statements)
  - Records pass as `Value::Array` parameters, skipping per-row
    `SurrealQL` parsing entirely.
- The previous inline-string emitters (`build_batch_sql`,
  `push_*_object`, `push_edges_grouped`, `push_quoted`,
  `push_edge_set_clauses`, `has_properties`) are removed (~215 LOC).
- Edge tables remain `TYPE RELATION`; `INSERT RELATION INTO` is
  `SurrealDB` 3.0's supported bulk-write shape for relation tables
  and produces the same row layout `RELATE` did. Read-side queries
  (kenn-mcp, kenn-store read paths) work unchanged.
- Wire format, snapshot layout, public APIs: no change.

## Capabilities

### New Capabilities

- `bound-parameter-ingest`: defines the contract that the rust-side
  `SurrealdbSink` writes records via parameter-bound `SurrealQL`,
  not inline-array literals.

### Modified Capabilities

None. The `index-store-db` capability defines the `Sink`
contract at the level of "ingests records per the streaming
contract"; query-construction shape is an implementation detail
this proposal pins down independently.

## Impact

- **Code**:
  - `crates/kenn-store/src/db.rs` — replaced `build_batch_sql` and
    its helpers with `build_batch_sql_bound` plus per-table
    `<table>_to_value` converters and `build_edge_batches`.
    `flush_batch` now binds each non-empty `Option<Value>` to its
    named parameter before executing the query.
  - `crates/kenn-store/Cargo.toml` — added `kv-mem` feature for
    backend-isolation benchmarks (probe-only; `RocksDB` remains
    the production backend).
  - `crates/kenn-indexer/src/transform_jsonl.rs` — added
    `ingest_jsonl_into_sink_threaded`: a reader thread feeds a
    bounded channel; the worker thread parses + sinks. Keeps the
    OS pipe drained so `kenn-dotnet` doesn't stall on
    `JsonlSink._sync` while the rust side is busy. Used by the
    pipeline.
  - `crates/kenn-indexer/src/pipeline.rs` — JSONL branch uses the
    threaded ingest. Added stage-timing instrumentation gated by
    `KENN_BENCH=1`.
  - `crates/kenn-cli/src/cmd_index.rs` — added stage-timing prints
    gated by `KENN_BENCH=1`, plus a `KENN_NULL_SINK=1` probe arm
    that swaps in a counting-only sink for measurement.
  - `scratch/bench/` — fixture (`app.jsonl`), replay shim
    (`replay-kenn-dotnet.sh`), bench-replay workspace
    (`replay-ws/`); `just bench-fixture-app` regenerates the
    fixture.
- **APIs**: kenn-cli surface, JSON event stream, kenn.toml schema,
  on-disk snapshot layout — all unchanged.
- **Schema**: unchanged. Tables, fields, indexes, and `TYPE
  RELATION` declarations are identical.
- **Performance** (app, 121 projects, 4235 files, 658k records):
  - `kenn index` wall: **98s → 67.65s** (~31% faster).
  - Replay (cat fixture | kenn): **61s → 35s** (~43% faster).
  - Snapshot stats unchanged: documents=4234, symbols=69145,
    edges=450764. Verified bit-equivalent ingest output.
