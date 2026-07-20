## REMOVED Requirements

### Requirement: Inference is single-worker with optional batch coalescing

**Reason:** Superseded by priority model-batch scheduling (see Added below). The old strictly-FIFO `mpsc` coalescing worker (`WorkItem`/`run_worker`/`process_batch`) is replaced by the shared `PriorityEmbedScheduler` from `embed-query-priority`, which drains high before low at `SEQS_PER_BATCH` granularity.

## ADDED Requirements

### Requirement: Inference is single-worker with priority model-batch scheduling

The embeddings capability SHALL run inference through a single
worker — at most one model encode is in flight at any moment — that
**reuses one resident model/context across batches** rather than per
request.

On accepting a `POST /v1/embeddings` request, the worker pipeline SHALL
process its `input` rows in **batches of at most the model's internal
batch size** (`SEQS_PER_BATCH`), classed **high** or **low** by the
request's priority (see the priority-classification requirement below).
The worker SHALL serve all ready **high** batches before the next
**low** batch. A large request therefore becomes many model-sized batches
rather than one long encode, and an interactive request is served after at
most the one batch currently in flight — never after a whole large bulk
request.

The worker MAY pack several small same-class requests into one batch when
multiple are waiting, but subject to **one bound: no batch SHALL exceed
`SEQS_PER_BATCH`**. It SHALL NOT build a larger batch (which would make an
interactive request wait for more than one model batch, breaking the
latency bound). Packing is **within a single priority class only** — a
high (query) batch SHALL NOT include low (bulk) rows. Each request's rows
are split/concatenated so each caller receives exactly its own vectors in
its own input order.

Batching SHALL preserve correctness: every caller receives exactly the
vectors for the inputs it sent, in the order it sent them, and
`usage.prompt_tokens` reflects only that caller's inputs.

The batch-loop / priority / drain logic SHALL be the **same shared
scheduler component** the in-process producer uses (see
`embedding-producer`), not a daemon-specific reimplementation — the daemon
runs it over its resident context and the `/v1/embeddings` handler retains
only HTTP concerns (classify priority from `input` shape, stream-parse the
request into the worker, stream-write the per-caller vectors + `usage` into
the standard OpenAI response — see the streaming requirement). This keeps
the two modes behaviorally identical by construction.

#### Scenario: two concurrent single-string requests both succeed

- **GIVEN** a running server with the model resident
- **WHEN** two `/v1/embeddings` requests arrive simultaneously, each with a single-string `input`
- **THEN** both succeed and return their respective vectors
- **AND** each response's `data` contains exactly one entry corresponding to that caller's input

#### Scenario: an interactive request is served ahead of a large bulk request

- **GIVEN** a large bulk `/v1/embeddings` request being processed batch-by-batch
- **WHEN** an interactive request arrives (marked high priority)
- **THEN** it is encoded after at most the one model batch currently in flight, not after the whole bulk request
- **AND** the bulk request still receives all of its vectors in input order

#### Scenario: coalesced batch preserves per-caller accounting

- **GIVEN** two concurrent requests A (`input: "alpha"`) and B (`input: ["beta", "gamma"]`) coalesced into one inference call within the same priority class
- **WHEN** the worker fans the results back
- **THEN** A's response contains exactly one entry for "alpha"
- **AND** B's response contains exactly two entries in order ["beta", "gamma"]
- **AND** A's `usage.prompt_tokens` counts only "alpha"; B's counts only "beta" + "gamma"

### Requirement: Priority is classified from the OpenAI input shape

The daemon SHALL classify a `POST /v1/embeddings` request's priority from
the **standard OpenAI `input` shape — no new field, no required header**: a
**single-string** `input` (`Input::One`) is a one-shot query ⇒ **high**; an
**array** `input` (`Input::Many`) is a batch ⇒ **low**. This matches kenn's
calls exactly — `embed_query` sends one string, the bulk pass sends arrays —
so priority is conveyed within the unmodified OpenAI request.

An optional `X-Kenn-Priority: interactive | bulk` header — the one
sanctioned kenn addition — MAY override the cardinality default; when
absent, cardinality decides. The header value, if present and recognized,
wins; an unrecognized value is ignored (fall back to cardinality). The
request/response JSON is unchanged either way.

#### Scenario: a single-string input is served as high priority

- **WHEN** a `/v1/embeddings` request arrives with a single-string `input` and no override header
- **THEN** it is classed high priority and served ahead of bulk batches

#### Scenario: an array input is classed bulk

- **WHEN** a `/v1/embeddings` request arrives with an array `input` and no override header
- **THEN** it is classed low (bulk) priority
- **AND** the response is byte-for-byte the standard OpenAI shape

#### Scenario: the optional header overrides cardinality

- **WHEN** a request carries `X-Kenn-Priority: interactive`
- **THEN** it is classed high regardless of `input` cardinality

### Requirement: The embeddings endpoint streams bytes without buffering the whole payload

The `/v1/embeddings` request and response **format SHALL remain the
standard OpenAI JSON** — same route, same shape. To serve server mode's
low-memory purpose (one model process, thin clients), the daemon SHALL
read the request body and write the response body as **byte streams**,
never materializing the whole payload:

- **Request**: stream-parse the JSON body, emitting each `input[i]` string
  as the array streams in and feeding it to the shared scheduler — the full
  `input` array is never all in memory.
- **Response**: write the standard OpenAI JSON incrementally (chunked) —
  the `{"object":"list","data":[` envelope, then each
  `{"embedding":[…],"index":i}` as its batch completes, then
  `],"model":…,"usage":…}` last (token counts accumulated as batches
  finish). The bytes on the wire are exactly the OpenAI response.

The daemon SHALL hold at most the scheduler's in-flight batch(es) plus
minimal parse/serialize buffering — not the full input or output set.
There SHALL be **no new endpoint, no new body field, and no non-OpenAI
format**; the only addition beyond stock OpenAI is the `X-Kenn-Priority`
header and the streamed (vs buffered) encode/decode.

Because the response is streamed, an encode failure that occurs **after**
the response body has begun cannot be reported with an HTTP status. In that
case the daemon SHALL **drop the connection** (abort the body); the client
sees a truncated/unparseable response and treats it as a failed request. A
failure **before** the first response byte SHALL still return a normal error
status.

#### Scenario: a large embed streams in and out without buffering the whole set

- **GIVEN** an `/v1/embeddings` request with many thousands of `input` entries
- **WHEN** the daemon processes it
- **THEN** `input` entries are parsed and fed to the scheduler incrementally
- **AND** each vector is written into the response `data` array as its batch completes
- **AND** the daemon holds at most the in-flight batch(es), not all inputs or all vectors
- **AND** the response body is byte-for-byte the standard OpenAI embeddings JSON

#### Scenario: query interleaves ahead of a large in-flight bulk

- **GIVEN** a large bulk `/v1/embeddings` (array `input`) streaming through the worker
- **WHEN** a query embed arrives as a single-string `input` request
- **THEN** it is classed high and served ahead of the remaining bulk batches (one shared scheduler)

#### Scenario: mid-stream encode failure drops the connection

- **GIVEN** a response that has already begun streaming (`200` + `data` prefix sent)
- **WHEN** an encode fails partway through
- **THEN** the daemon drops the connection rather than emitting a status code
- **AND** the client observes a truncated/unparseable body and treats the request as failed
