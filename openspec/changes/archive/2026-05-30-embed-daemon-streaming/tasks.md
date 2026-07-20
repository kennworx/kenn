# Tasks — embed-daemon-streaming

(Depends on `embed-query-priority` for the shared `PriorityEmbedScheduler`.)

## 1. Priority classification (D1)
- [x] 1.1 Daemon `/v1/embeddings` classifies priority from `input` shape: `Input::One(String)` ⇒ high, `Input::Many` ⇒ low (`crates/kenn-server/src/embeddings.rs` — `classify_priority`); optional `X-Kenn-Priority` header overrides (`PRIORITY_HEADER`); unknown/absent ⇒ cardinality; request/response shape unchanged

## 2. Worker = shared scheduler (D1)
- [x] 2.1 Replaced the strictly-FIFO `mpsc` worker (`WorkItem`/`run_worker`/`process_batch`) with the shared `PriorityEmbedScheduler` over a `LlamaBatchEncoder` (production) / `ProducerBatchEncoder` adapter (tests). Verified: 28/28 kenn-server tests pass.
- [x] 2.2 No scheduling logic duplicated in `kenn-server` — the handler dispatches through `module.scheduler.submit`; the batch-loop/queue/drain code exists only in `kenn-embed::scheduler`.

## 3. Streamed standard-OpenAI byte I/O (D2) — deferred follow-up
- [ ] 3.1 Stream-parse the request `input` entries into the scheduler input stream as they arrive (no whole-body buffer) → currently the handler uses axum's `Json<EmbeddingsRequest>` extractor which buffers the body. The priority/scheduler fix (1.1/2.1) is delivered; incremental JSON parsing of an arbitrary-ordered object is a substantial axum/serde refinement to schedule separately.
- [ ] 3.2 Stream-write the response `{object,data:[…],model,usage}` incrementally — defer; current response is buffered via `Json(build_response(...))`. Same scheduling note as 3.1.

## 4. Mid-stream error (D3) — deferred (no streaming response yet)
- [ ] 4.1 After the response body has begun, an encode failure drops the connection — N/A until 3.2 lands.

## 5. Gates
- [x] 5.1 `cargo clippy --workspace --all-targets` clean; `just crap-ci` PASSED; `cargo fmt --all` — verified before commit `acb9730` and through subsequent gates.
- [x] 5.2 Live verified end-to-end via the in-process scheduler (which uses the same `PriorityEmbedScheduler` the daemon does); the `high_priority_interleaves_with_a_large_low_job` unit test confirms cardinality-priority preemption at `SEQS_PER_BATCH` granularity. Streamed-bytes I/O (3.x) deferred separately.
