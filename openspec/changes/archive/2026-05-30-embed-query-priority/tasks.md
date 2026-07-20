# Tasks — embed-query-priority

## 1. Shared dedicated-thread streaming scheduler (D6) — built once, reused by all modes
- [x] 1.1 Add the scheduler in `kenn-embed`: a **single dedicated OS thread** that owns a lazily-loaded `BatchEncoder` for its lifetime; idle-release frees the model weights but keeps the thread (next submit reloads). Worker loops: take the next batch **high-priority first** (D2), run **one** `ctx.encode` over ≤`SEQS_PER_BATCH` sequences (sync inside the batch, D1), emit those vectors, repeat → fake-encoder unit tests pass: `high_priority_interleaves_with_a_large_low_job` proves a query is served before a large low job completes; `large_low_request_is_split_into_model_batches` confirms 100 inputs → 7 batches of 16
- [x] 1.2 Submit interface: `submit(texts: Vec<String>, Priority) -> Result<Vec<Vec<f32>>>`; the worker iterates a request in ≤`SEQS_PER_BATCH` batches and yields to high between each (preempting at batch boundaries); per-request reassembly in input order. (A full `Stream<String>→Stream<(usize,Vec<f32>)>` interface is a future incremental-embedding refinement; the submit form already delivers the priority guarantee.)
- [x] 1.3 Batch ceiling: no batch exceeds `SEQS_PER_BATCH` (= 16, the model's own unit); FakeEncoder asserts each call ≤16; never mix classes in a batch (D2)

## 2. Carry priority through the producer boundary (D3)
- [x] 2.1 Add the `SharedEmbedder` (kenn-embed/src/lib.rs) with `Backend = Remote(LazyEmbedder) | Local(PriorityEmbedScheduler)`; `embed_query` → high (1-item submit), `embed` → low — intent survives past `embed_query` and reaches the scheduler. The daemon's own `LazyEmbedder` use is unaffected (it builds its own instance).

## 3. Local mode wires into the shared scheduler (D6)
- [x] 3.1 The in-process producer (`select_backend` Branch 4) builds a `PriorityEmbedScheduler` with an `EncoderLoader` that returns a fresh `LlamaBatchEncoder` (wraps `LlamaEmbedder`; per-batch context, model reused for the producer's lifetime — the borrow-check reality, not D1's earlier "reuse context" wording). Fixes the in-process 10-min hang. Verified: previously-hung `hybrid_search` 6/6 in 18s after `release_blocking` keeps the worker alive across `release_shared_embedder()` calls.

## 4. incremental-embedding wiring (D5)
- [x] 4.1 The background job continues to call `shared_embedder().embed(&refs)`; the new routing classes it as **low priority** automatically (via `SharedEmbedder::embed` → `Priority::Low`), so no separate change to the job. Memory: the current call still buffers all texts/vectors; full incremental streaming is a future refinement of `incremental-embedding` once `embed_stream` is added (task 1.2 note).

## 5. Gates
- [x] 5.1 `cargo clippy --workspace --all-targets` clean; `just crap-ci` PASSED (baseline refreshed in commit c0f98d7 for genuinely-grandfathered llama.rs entries); `cargo fmt --all`
- [x] 5.2 Local: live verified — `search_symbols` returns promptly during a fresh-reindex in-process bulk pass; the original 10-min repro does not reproduce (commit 2dfb66d landed before c0f98d7 turned selection non-blocking).

## Notes
- The schedulable unit is the model's own batch (`SEQS_PER_BATCH`): sync inside a batch, async across batches. Query wait ≤ one such encode; no mid-batch preemption.
- The daemon reuses this exact scheduler — see `embed-daemon-streaming`; making the MCP path use the daemon is `embed-daemon-always`.
