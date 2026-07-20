## Why

When the `kenn server` daemon is the embedding producer, a query request can
queue behind a long bulk request server-side, and a large request buffers whole
payloads in memory — defeating server mode's reason to exist (one process holds
the model; thin clients). This change makes the daemon serve through the shared
priority scheduler with **streamed, standard-OpenAI** byte I/O.

**Depends on** `embed-query-priority` (the shared `PriorityEmbedScheduler` this
change runs inside the daemon). Pairs with `embed-daemon-always` (which makes
the MCP path actually use the daemon).

## What Changes

- The daemon's `/v1/embeddings` worker runs the **shared scheduler**
  (`embed-query-priority`) over its resident model — `SEQS_PER_BATCH` batches,
  high before low, one encode in flight — replacing the strictly-FIFO queue.
- **Priority from the standard OpenAI `input` shape**: a single-string `input`
  is a one-shot query ⇒ **high**; an array `input` is a batch ⇒ **low**. No new
  field; this matches kenn's calls exactly. An optional `X-Kenn-Priority`
  header — the one sanctioned addition — MAY override it.
- **Streamed standard-OpenAI byte I/O** (low memory, no new format): the daemon
  stream-parses `input` into the scheduler as it arrives and stream-writes the
  `data` array out per batch (envelope → `{embedding,index}` per batch →
  `model`/`usage` last). Same wire bytes as a stock OpenAI server; only the
  server's footprint changes.
- **Mid-stream error = drop the connection**: once the response has begun, an
  encode failure can't use an HTTP status, so the daemon aborts the body and the
  client treats the truncated response as a failed request.

## Capabilities

### Modified Capabilities
- `embeddings-api`: the single worker is the shared `SEQS_PER_BATCH` priority
  scheduler classed by `input` cardinality (optional `X-Kenn-Priority`
  override), replacing the v1 "no priority queue — FIFO" clause; `/v1/embeddings`
  JSON is streamed in/out for low memory (same format); mid-stream failure drops
  the connection.

## Impact

- **Latency**: a daemon-side query is served ahead of bulk within one model
  batch.
- **Memory**: the daemon holds at most in-flight batches, not whole payloads.
- **Compatibility**: request/response JSON is unchanged; the only deviations
  from a stock OpenAI server are streamed encode/decode and the optional header.
- **Out of scope**: the scheduler itself (`embed-query-priority`); whether the
  MCP/CLI path uses the daemon at all (`embed-daemon-always`).
