## Why

An interactive `search_symbols` MCP call hung **~10 minutes** ("in progress",
no approval prompt) right after a cold reindex. Root cause (process sample +
code trace): the cold reindex left ~7200 symbols unembedded, so the background
embed job called `shared_embedder().embed(&refs)` with **all ~7200 texts in one
`spawn_blocking`** (`crates/kenn-store/src/db/lance/store.rs:211`) — one
unyieldable multi-minute encode. A concurrent query embed
(`shared_embedder().embed_query(...)`, `crates/kenn-store/src/db/reader.rs:460`)
**starved** behind it. `get_index_status` (pure atomics) stayed instant — this
is purely the embedding path.

This change introduces the **shared priority scheduler** that fixes the
in-process contention. Two follow-on changes apply it to the daemon
(`embed-daemon-streaming`) and make the MCP path use the daemon
(`embed-daemon-always`).

## What Changes

- **Batch at the model's own unit, yield between batches.** `embed()` is not
  one-shot — it loops at `SEQS_PER_BATCH` (=16) sequences per `ctx.encode` inside
  one unyieldable blocking call. The fix moves that loop into a **single
  dedicated inference thread** (reusing one resident context) that runs **one
  ≤`SEQS_PER_BATCH` encode at a time** and re-checks the high-priority class
  **between every batch** — sync inside a batch, async across batches. A query
  waits at most one model batch, the tightest bound the model allows.
- **Two priority classes**: interactive query embeds (high) are served ahead of
  background bulk (low) at batch boundaries; packing never exceeds one batch.
- **Streaming interface** (texts in → vectors out, batch-by-batch): query is a
  1-item stream; the bulk pass streams and consumes vectors incrementally.
- **One shared component**: the scheduler lives in `kenn-embed` and is reused by
  the daemon (`embed-daemon-streaming`) and the in-process fallback — identical
  behavior by construction.

## Capabilities

### Modified Capabilities
- `embedding-producer`: carries an interactive-vs-bulk priority through the
  boundary; a single dedicated inference thread reuses one resident context and
  runs one ≤`SEQS_PER_BATCH` encode at a time, serving an interactive query
  ahead of bulk within at most one model batch.
- `incremental-embedding`: the background job submits its misses as **bulk
  (low) priority** and consumes the result stream incrementally to bound memory;
  the published sidecar segment stays **atomic** (append + hot-swap), not torn.

## Impact

- **Latency**: an interactive query embed returns within ~one model batch even
  while a large background embed runs — no multi-minute hangs.
- **Throughput**: the bulk pass total time is essentially unchanged (same texts,
  same internal 16-seq batches), just yielding between them.
- **Out of scope**: the daemon's HTTP streaming protocol
  (`embed-daemon-streaming`) and whether the MCP/CLI path uses the daemon
  (`embed-daemon-always`); the embedding model, sidecar format, reconciliation.
