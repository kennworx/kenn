# Tasks — stream-reembed-pipeline

## 1. RemoteEmbedder client-side chunking
- [x] 1.1 Add `const REMOTE_CHUNK: usize = 256;` in `crates/kenn-embed/src/remote.rs`
- [x] 1.2 Extract the current body of `RemoteEmbedder::embed` into a private `embed_one_request(&self, texts: &[&str])` method (verbatim) → verify: unit tests still pass
- [x] 1.3 Rewrite `embed` as a `for chunk in texts.chunks(REMOTE_CHUNK)` loop calling `embed_one_request` and extending `out` → verify: existing remote tests pass; output order preserved
- [x] 1.4 New unit test: 600-input call produces 600 vectors in input order, with the FakeServer asserting it received exactly 3 requests (256+256+88) → verify: passes
- [x] 1.5 Error short-circuit: a chunk failure aborts the call with the chunk's `EmbedError` (no partial result) → verify: test with a server that fails the 2nd request returns Err

## 2. Stream the Lance scan in reembed_batches
- [x] 2.1 Rename `reembed_batches` → `reembed_into(&self, build: &LanceStore)` returning `(vectors: usize, embed_seconds: f64)` → verify: signature compiles
- [x] 2.2 Split into two scan passes:
  - Pass 1: stream the scan, accumulate `HashMap<u32, String>` via `doc_text_by_short_id` row-by-row → verify: map size matches non-empty docs
  - Pass 2: stream the scan again; per batch, run `name_row_embed_texts(batch, &doc_by_id)` → embed → `apply_embeddings` → `build.append_batch(new_batch)` → verify: works on existing test fixtures
- [x] 2.3 Wall-clock `embed_seconds`: only the time spent in `embed_block_until_ready` (sum across batches) → verify: matches pre-change semantics (the producer cost, not the apply/append cost)
- [x] 2.4 Update the caller in `crates/kenn-store/src/db/mod.rs::reembed` to open the build store first, pass it in, then `temp.finalize()` + `publish_swap` directly → verify: integration test (full reembed against a real Lance store) round-trips

## 3. Stream the Lance scan in embed_pending_batches
- [x] 3.1 Rename `embed_pending_batches` → `embed_pending_into(&self, build: &LanceStore)` returning `(new_entries, live, embed_seconds)` → verify: signature compiles
- [x] 3.2 Two-pass like §2.2, plus per-batch accumulation of `live: HashSet<u64>` and `new_entries: Vec<(u64, Vec<f32>)>` → verify: matches pre-change return values
- [x] 3.3 Update the caller in `crates/kenn-store/src/db/mod.rs::embed_pending` to write to `build` per batch and pass through the report → verify: incremental embed integration test passes
- [x] 3.4 Confirm sidecar segment append still receives `new_entries` in the same order it would have before → verify: existing sidecar tests pass

## 4. Drop the rebuild_knowledge wrapper
- [x] 4.1 Inline the `for batch in rebuilt { temp.append_batch(batch) }` loop into the two streaming flows (now happens inside §2.4 / §3.3) → verify: no remaining callers of `rebuild_knowledge`
- [x] 4.2 Delete `rebuild_knowledge` from `crates/kenn-store/src/db/mod.rs` → verify: `cargo build --workspace` clean

## 5. Tests + gate
- [x] 5.1 Memory smoke: confirm peak memory during a real reindex is bounded by `scan_batch_size + REMOTE_CHUNK` by tracking `Vec<RecordBatch>` no longer appears in either flow → verify: source-only assertion (no allocation profiler needed)
- [x] 5.2 `cargo test --workspace` green → verify: full suite
- [x] 5.3 `cargo clippy --workspace --all-targets` zero warnings → verify
- [x] 5.4 `just crap-ci` green; if new functions trip the gate, refactor or refresh baseline with rationale → verify
- [x] 5.5 End-to-end smoke: `./build/kenn index --workspace . --force` against this repo; embed pass runs; vector count matches pre-change → verify: identical row counts in `lance/knowledge/` between an old build and the streaming build
- [x] 5.6 `cargo fmt --all` as final step → verify
