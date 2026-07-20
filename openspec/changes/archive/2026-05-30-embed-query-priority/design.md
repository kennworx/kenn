# Design — query-priority embedding scheduler (in-process)

This change is the **shared scheduler** + the in-process fix for the reported
query stall. The daemon's use of this scheduler over HTTP is
`embed-daemon-streaming`; making the MCP path use the daemon at all is
`embed-daemon-always`. The scheduler defined here is what both reuse.

## Evidence (the 10-minute hang)

```
  search_symbols ─► with_db (brief lifecycle.read(), dropped before await)
                  ─► search_blended_hits
                  ─► shared_embedder().embed_query(q)      reader.rs:460   ← starved
  spawn_embed_job ─► embed_pending ─► embed_pending_batches
                  ─► shared_embedder().embed(&refs)        store.rs:211    ← ~7200 texts, ONE spawn_blocking
```

`get_index_status` is pure atomics (no embed) → instant throughout. The hang
is the **query-embed** starving behind the **bulk-embed** on a single
serialized inference resource.

## Two mechanisms, both required

Inference is serialized — one `ctx.encode` in flight at a time. A priority
scheme can only reorder work **at encode boundaries**. So two things are needed,
and **the processor owns both**:

1. **Batch at the model's own unit.** `embed()` already loops at
   `SEQS_PER_BATCH` (=16) sequences per `ctx.encode`, but inside one unyieldable
   blocking call (model internals, below). The fix runs **one ≤`SEQS_PER_BATCH`
   encode at a time** with a boundary after each — *regardless of how large a
   single request is* — so a 7200-text request becomes many short encodes
   instead of one multi-minute call.
2. **Two priority classes.** Between batches the processor serves all ready
   **high**-priority (interactive query) work before the next **low** (bulk)
   batch. A query thus waits at most one model batch.

## Decisions

> **The natural unit is one `SEQS_PER_BATCH` encode (model internals).**
> `LlamaEmbedder::embed(&texts)` (`kenn-embed/src/llama.rs:113`) is *not*
> one-shot: it tokenizes all inputs, then loops packing up to `SEQS_PER_BATCH`
> (=16) sequences per `ctx.encode` until every text is done
> (`llama.rs:148-174`) — a 7200-text call runs ~450 internal encodes, all inside
> **one synchronous blocking call with no yield points**. So the indivisible
> unit the GPU runs is one `ctx.encode` of ≤`SEQS_PER_BATCH` sequences. The
> design adopts exactly that as the schedulable unit: **sync inside a batch,
> async across batches** — the batch loop moves out of `embed()` into the
> scheduler worker so it can re-check the high-priority queue between encodes.

### D1. The schedulable unit is one `SEQS_PER_BATCH` batch; the batch loop lives in the worker
The processor SHALL process inference in units of **one `ctx.encode` over
≤`SEQS_PER_BATCH` (=16) sequences** — the model's own internal batch size, not
an arbitrary 128. The per-batch loop that today lives inside
`LlamaEmbedder::embed` SHALL move into the scheduler worker: the worker picks
the next batch (high first, D2), runs **one** synchronous `ctx.encode`, fans
its vectors back, then loops. A request's rows are split across as many
≤16-seq batches as needed and reassembled in input order before its reply.

**Model reused across batches; context per batch (borrow-check reality).** The
worker holds **one resident model** for the producer's lifetime — that is the
expensive load. The `LlamaContext` borrows the model (`LlamaContext<'a>`), so a
struct holding both is self-referential and not expressible in safe Rust
without `ouroboros`. The implementation therefore reuses the **model** (kept on
the dedicated thread) and creates a fresh **context per ≤16-seq batch** inside
`LlamaBatchEncoder::encode_batch`. Context allocation is small relative to the
encode, and a query is a single batch, so this only adds minor overhead to the
background bulk pass — the query latency bound (D4) is unaffected. Truly reusing
a context via `ouroboros` is a future perf refinement, not required by the
hang fix.

**Reassembly bookkeeping.** Each batch carries `(request_id, row_range)` so the
worker fans results back to the right caller/offset, completing a request's
`oneshot` only when all its batches are done.

### D2. The worker drains high before low, between every batch
Between each ≤16-seq `ctx.encode` the worker SHALL serve all ready **high**
batches before the next **low** batch. So a query enqueued while a large bulk
request's batches wait is encoded **next** — after at most the one ≤16-seq batch
currently in flight. **One bound governs splitting and packing: no batch ever
exceeds `SEQS_PER_BATCH`** — packing several small same-class requests fills a
batch only up to that ceiling, never beyond (a larger batch would make a query
wait more than one encode, voiding D4). Packing is **within one priority class
only**, never mixing a query into a bulk batch.

### D3. Priority intent at the producer boundary
Today the intent is **lost**: `LazyEmbedder::embed_query` collapses to
`embed(&[text])` (`kenn-embed/src/lib.rs:196`). The producer boundary SHALL
distinguish **interactive** vs **bulk** embeds (a priority argument or a
dedicated `embed_query`), default **bulk**, so the worker can class the request.
(How the daemon conveys this over HTTP is `embed-daemon-streaming`; here it is
an in-process API concern.)

### D4. Query wait is bounded to one `SEQS_PER_BATCH` encode
A query arriving while a low batch is mid-`ctx.encode` waits for that batch (no
mid-encode preemption — a single `ctx.encode` is not interruptible), then is
served before the next low batch. Worst-case added query latency ≈ one ≤16-seq
encode — the model's atomic unit. **Accepted**: no mid-batch preemption (and
none is needed, since the batch *is* the atomic unit).

### D5. Client-side bulk pre-chunking is unnecessary
The worker batches at `SEQS_PER_BATCH` regardless of how the caller feeds it, so
`incremental-embedding`'s bulk job no longer pre-chunks for responsiveness — it
submits its misses **low priority** and SHOULD consume the result stream
incrementally to bound memory (the published sidecar segment stays atomic — see
`incremental-embedding`).

### D6. One shared scheduler: a dedicated inference thread with a streaming interface
The batch-loop + hi/lo-queues + drain logic SHALL live in **one reusable
component** in `kenn-embed`, consumed by every mode — no second copy.

**A dedicated inference thread, not an async callback.** Because the loop reuses
one resident `LlamaContext` across encodes (D1) and a context is not `Send` /
safe to shuttle across `spawn_blocking` calls, the worker is a **single
dedicated OS thread** that owns the context for its lifetime. It does the
`ctx.encode` **synchronously**; callers talk to it over channels and `await`
(async across batches).

**Streaming interface.** The schedulable shape is a stream — feed texts, receive
vectors, processed `SEQS_PER_BATCH` at a time — which composes with the batch
loop and gives bulk callers memory-bounded incremental results:

```
  fn embed_stream(pri: Priority, inputs: impl Stream<Item = String>)
        -> impl Stream<Item = (usize, Vec<f32>)>
```

- **Worker loop (dedicated thread)**: pull up to `SEQS_PER_BATCH` items — high
  stream first, then low — run one `ctx.encode`, emit those vectors, repeat.
  Lazy-load on first batch; **idle-release frees the model weights but keeps the
  thread** (it re-loads lazily on the next batch).
- **Query** (`embed_query`): a 1-item high-priority stream → one vector.
- **Bulk** (`incremental-embedding`): a low-priority stream, vectors consumed as
  they arrive.

The daemon (`embed-daemon-streaming`) reuses this same component over its
resident context — identical behavior by construction, one test suite.

## Tradeoffs / risks
- **No mid-batch preemption** (D4): a query waits at most one ≤`SEQS_PER_BATCH`
  encode — the model's atomic unit, so the bound is as tight as the model allows
  and needs no config knob.
- **Dedicated OS thread**: needed because the resident `LlamaContext` is reused
  across batches and isn't safe to move across `spawn_blocking`. The thread
  persists for the producer's life; **idle-release frees the weights, not the
  thread**.
- **Starvation of bulk under heavy query load**: acceptable — interactive
  freshness wins; the bulk catch-up resumes when queries quiesce.

## Alternatives considered
- **Arbitrary sub-chunk size (e.g. 128) instead of `SEQS_PER_BATCH`**: a unit
  larger than the model's internal batch is still one unyieldable `embed()`
  multi-encode; aligning to `SEQS_PER_BATCH` makes the schedulable unit the
  model's own atomic encode and tightens the query bound. Adopted.
- **`spawn_blocking` per batch (no dedicated thread)**: can't reuse a non-`Send`
  `LlamaContext` across pool threads → would recreate the context ~450× per
  pass. Rejected for the dedicated thread.
- **No priority, fair scheduling only**: fairness ≠ strict priority; under a
  steady bulk stream a query could wait several batches. Rejected.
- **Separate model instance for queries**: doubles memory, still contends on the
  GPU; rejected.

## Build order
```
  1. Dedicated-thread streaming scheduler in kenn-embed (D6): one resident
     context, batch loop at SEQS_PER_BATCH (D1), hi/lo priority streams drained
     high-first between batches (D2), per-request reassembly. Unit-tested over a
     fake encoder.
  2. Producer boundary carries priority (D3): interactive/bulk distinction so
     intent survives past embed_query; the in-process path feeds the scheduler.
     Fixes the 10-min hang.
  3. incremental-embedding: submit misses low priority, consume the stream
     incrementally, publish the segment atomically (D5).
```
