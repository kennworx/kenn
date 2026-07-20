## Why

The reembed pass (the `kenn update` embedding pass and the incremental `embed_pending` job) buffers the **entire knowledge store** in memory before calling the embedder:

```rust
let batches: Vec<RecordBatch> = scan
    .try_into_stream().await?
    .try_collect().await?;       // <-- all row groups, eager
let texts: Vec<String> = ...;    // <-- every name-row's embed text
let vectors = embed(&refs).await?; // <-- 4k+ strings in one call, 4k+ vectors back
```

The Lance scan API is already a `Stream<Item=RecordBatch>` — we just collect it. The producer trait takes `&[&str]` and returns `Vec<Vec<f32>>`, so even though the in-process scheduler chunks at `SEQS_PER_BATCH=16` internally and the remote daemon's server-side scheduler does the same, the **HTTP request** between client and daemon carries the full text array in one POST, and the **client-side accumulator** holds every batch + every text + every vector simultaneously.

For the current repo (~4k symbols) the buffered shape works, but it scales linearly with corpus size and has two concrete failure modes at scale:

- **HTTP timeout**: `RemoteEmbedder` has a 60s reqwest timeout. A 50k-symbol reindex against a slow model would exceed it.
- **Peak memory**: all RecordBatches + Vec<String> + Vec<Vec<f32>> co-resident. At 50k symbols × 768-dim f32 vectors, the response alone is ~150 MB.

The OpenAI `/v1/embeddings` schema has no streaming variant (no `stream: true`), so the wire protocol can't change. But the pipeline doesn't need protocol streaming — it needs to stop pre-buffering on both sides of the producer call.

## What Changes

- **Stream the Lance scan**. `reembed_batches` and `embed_pending_batches` stop calling `try_collect()`. They consume `Stream<Item=RecordBatch>` one batch at a time.
- **Two-pass scan** (forced by the existing schema). Pass 1 builds the doc lookup (`HashMap<u32, String>` keyed by `short_id`) — small, just doc text strings. Pass 2 streams `RecordBatch`s; per batch, extract name-row texts → embed → `apply_embeddings` → append to the build store. The first pass is necessary because `name_row_embed_texts` joins a name row with its symbol's doc text, and the two rows can live in different scan batches (linked only by `short_id`).
- **Write to the build store as we go**. `reembed_batches` and `embed_pending_batches` change from returning `Vec<RecordBatch>` to accepting a `&LanceStore` build target and writing per-batch. The `rebuild_knowledge` helper collapses into the caller (it was only a `for batch in rebuilt { append_batch }` loop).
- **Client-side request chunking inside `RemoteEmbedder`**. The producer trait signature stays `fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>`, but the slice passed in is now bounded by Lance's `scan.batch_size(8192)`. Inside `RemoteEmbedder::embed`, split into `EmbeddingsConfig::batch_size` slices (default 256) and POST each as its own request; concatenate the results in input order. The in-process scheduler already chunks at 16 internally, no change there.
- **New config knob**: `[embeddings].batch_size: usize` (default 256) in the global `kenn-config`. Threaded from `select_backend` → `RemoteEmbedder::new`. A value of 0 falls back to the built-in default 256.

## Capabilities

### Modified Capabilities
- `knowledge-store-embed`: the `reembed` and `embed_pending` flows are streaming; their internal helpers (`reembed_batches`, `embed_pending_batches`) move from "collect → embed → return batches" to "stream → embed-per-batch → append-per-batch → return report".
- `remote-embedder`: `RemoteEmbedder::embed` is now multi-request — it slices its `&[&str]` input into chunks of 256 and concatenates results, transparently to callers. A single-chunk input is one HTTP request (unchanged shape).

## Impact

- **Memory at scale**: peak drops from `O(corpus_size)` to `O(scan_batch_size + REMOTE_CHUNK)`. At 50k symbols, the worst-case in-flight set is one Lance row group (≤8192 rows) + one 256-text response — order of magnitude smaller.
- **HTTP timeout safety**: each request is ≤256 inputs ≈ 16 scheduler batches ≈ ~1s on M-series. Well below the 60s timeout regardless of corpus size.
- **Wire format**: unchanged. OpenAI-compatible `/v1/embeddings` schema is honored as-is. Hosted OpenAI / ollama / lm-studio all stay compatible (typical hosted input cap is 2048; we send 256).
- **Producer trait**: unchanged. Tests using `FakeProducer` / `TrickyProducer` need no signature updates.
- **`rebuild_knowledge` helper deleted**: was a thin loop; the streaming version writes directly. Caller in `db/mod.rs` shrinks.
- **Two-pass scan cost**: one extra read of the Lance dataset. Lance reads are mmap'd + columnar; the doc-only pass touches `id`, `row_kind`, `doc_text` columns and skips everything else. Negligible vs the embed work itself.
- **Error semantics**: unchanged. A chunk failure inside `RemoteEmbedder::embed` short-circuits and returns the same `EmbedError` shape as today — `embed_block_until_ready` retries on `Starting`, all else fails the pass. No partial-vector accounting.
- **Out of scope**: protocol-level streaming (NDJSON/SSE — OpenAI schema doesn't permit); changing scheduler batch size (`SEQS_PER_BATCH=16` stays — that's the model's physical batch unit, a different knob).
