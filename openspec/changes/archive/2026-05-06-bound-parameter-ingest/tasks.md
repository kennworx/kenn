## 1. Bound-parameter sink

- [x] 1.1 Replace `build_batch_sql` and its inline-string helpers
      with `build_batch_sql_bound` that emits
      `INSERT INTO <table> $<table>` per record table and
      `INSERT RELATION INTO <kind> $b_<kind>` per edge kind.
- [x] 1.2 Add per-table value builders: `files_to_value`,
      `packages_to_value`, `symbols_to_value`,
      `symbol_docs_to_value`, `defs_to_value`. Each returns a
      `Value::Array` of `Value::Object` rows mirroring the
      schema's field set.
- [x] 1.3 Add `build_edge_batches`: groups edges by kind in the
      declared `EdgeKind` order (deterministic SQL across runs),
      emits one `INSERT RELATION INTO <kind> $b_<kind>` per
      non-empty kind, returns the bound `(param_name, Value::Array)`
      pairs.
- [x] 1.4 Update `flush_batch` to bind every non-empty table /
      edge-kind parameter via `.bind((name, value))` before
      executing the query.
- [x] 1.5 Delete dead inline-string emitters: `build_batch_sql`,
      `push_file_object`, `push_package_object`,
      `push_symbol_object`, `push_symbol_docs_object`,
      `push_def_object`, `push_edges_grouped`, `has_properties`,
      `push_edge_set_clauses`, `push_quoted` (~215 LOC).

## 2. Threaded ingest

- [x] 2.1 Add `transform_jsonl::ingest_jsonl_into_sink_threaded`:
      spawns a reader thread reading lines from a `BufReader`
      into a bounded `mpsc::sync_channel`; the worker thread
      parses + sinks. Owns the `Read` source.
- [x] 2.2 Factor the per-frame match arm out of
      `ingest_jsonl_into_sink` into a shared `handle_frame`
      helper used by both the sync and threaded variants.
- [x] 2.3 Switch `pipeline.rs::ingest_jsonl_subprocess` to use
      the threaded path; remove the now-unused
      `ingest_jsonl_into_sink` import and `BufRead`/`BufReader`
      imports.

## 3. Bench instrumentation

- [x] 3.1 Add stage timings in `pipeline.rs::run_pipeline` (total,
      streaming, flush_stubs, end_run) gated by `KENN_BENCH=1`.
- [x] 3.2 Add stage timings in `cmd_index.rs` (sink_open,
      run_pipeline_total, persist, publish, gc) gated by
      `KENN_BENCH=1`.
- [x] 3.3 Add `KENN_NULL_SINK=1` arm in `cmd_index.rs` that swaps
      the `SurrealdbSink` for a record-counting `NullSink` (no
      persist/publish). Used to isolate parse+transform cost
      from `SurrealDB` write cost.
- [x] 3.4 Add `kv-mem` feature to `surrealdb` in
      `crates/kenn-store/Cargo.toml` plus a
      `KENN_BENCH_MEM_DB=1` arm in `SurrealdbSink::open` that
      uses the in-memory engine. Used to confirm `RocksDB`
      isn't the bottleneck.

## 4. Bench fixture

- [x] 4.1 Capture a representative app JSONL stream at
      `scratch/bench/app.jsonl` (96 MB, 728k frames). The path
      lives under the gitignored `scratch/` so it persists
      locally without bloating the repo.
- [x] 4.2 Add `just bench-fixture-app` recipe that regenerates
      the fixture by running `kenn-dotnet` against
      the production workspace with all 3 configured
      `.sln`s.
- [x] 4.3 Add a replay-shim (`scratch/bench/replay-kenn-dotnet.sh`)
      that emits the captured fixture, plus a fixture workspace
      (`scratch/bench/replay-ws/`) with a `kenn.toml` pointing
      at it. Used to benchmark the rust side in isolation from
      `kenn-dotnet`.

## 5. Tests + validation

- [x] 5.1 Update the `build_sql_groups_edges_by_kind` test to
      assert the new bound-parameter shape:
      `INSERT RELATION INTO <kind> $b_<kind>` statements +
      one `(param, value)` pair per non-empty kind.
- [x] 5.2 `cargo test --workspace` passes (all suites green).
- [x] 5.3 `cargo clippy --workspace --all-targets` clean (only
      pre-existing warnings remain — renamed-lint and
      kenn-mcp/kenn-cli dep wiring).
- [x] 5.4 End-to-end measurement on the production workspace:
        - `kenn index` wall: 98s → 67.65s (−31%)
        - replay (`cat fixture | kenn`): 61s → 35s (−43%)
        - snapshot stats unchanged: documents=4234,
          symbols=69145, edges=450764
- [x] 5.5 No orphan kenn-dotnet/MSBuild/BuildHost processes
      after run.
