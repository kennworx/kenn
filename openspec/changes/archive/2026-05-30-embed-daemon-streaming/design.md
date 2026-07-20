# Design — daemon serves via the shared scheduler, streamed OpenAI bytes

Builds on `embed-query-priority`'s shared `PriorityEmbedScheduler` (one
dedicated inference thread, `SEQS_PER_BATCH` batches, high drained before low).
This change runs that scheduler inside the `kenn server` daemon and wires it to
the HTTP boundary without leaving standard OpenAI.

## Decisions

### D1. Priority from the standard OpenAI `input` shape
The daemon SHALL classify priority from the **unmodified OpenAI request**: a
**single-string** `input` (`Input::One`) is a one-shot query ⇒ **high**; an
**array** `input` (`Input::Many`) is a batch ⇒ **low**. OpenAI already
distinguishes these and it maps onto kenn's calls (`embed_query` sends one
string; the bulk pass sends arrays), so priority needs **no protocol addition**.
An optional `X-Kenn-Priority: interactive | bulk` header — the one sanctioned
kenn addition — MAY override the cardinality default; absent/unknown ⇒
cardinality. The request/response JSON is unchanged either way.

(An external client doing a genuine multi-input *interactive* request would be
classed low — acceptable; the daemon primarily serves kenn, and a low-classed
request is still served, just behind queries. It can set the header if it cares.)

### D2. Streamed byte I/O of the *standard* OpenAI JSON
Server mode exists so **one** process holds the model and others are thin
clients; buffering whole payloads defeats that. The daemon SHALL keep the **same
OpenAI `/v1/embeddings` JSON request/response** but read/write the bodies as
**byte streams**:

- **Request**: stream-parse the JSON body, emitting each `input[i]` string as
  the array streams in and feeding the scheduler — the full `input` is never all
  in memory.
- **Response**: write the standard OpenAI JSON incrementally (chunked) — the
  `{"object":"list","data":[` envelope, then each `{"embedding":[…],"index":i}`
  as its batch completes, then `],"model":…,"usage":…}` last (token counts
  accumulated as batches finish, where `usage` already sits in the JSON).

The daemon SHALL hold at most the scheduler's in-flight batch(es) plus minimal
parse/serialize buffering. **No NDJSON, no second endpoint, no new body field** —
the only deviations from a stock OpenAI server are streamed encode/decode and
the optional `X-Kenn-Priority` header. The handler keeps only HTTP concerns
(classify, stream-parse, stream-write, per-caller `usage`); all scheduling is
the shared component.

### D3. Mid-stream error drops the connection
Once the response has begun (status `200` + the `{"data":[` prefix on the wire),
an encode failure can no longer be signalled with an HTTP status. The daemon
SHALL **drop the connection** (abort the body); the client sees a truncated /
unparseable response and treats it as a failed request (retry / fallback). A
failure **before** the first byte SHALL still return a normal error status.

## Tradeoffs / risks
- **Streamed JSON parse/serialize is fiddlier than buffer-then-(de)serialize** —
  the daemon parses `input` and emits `data` incrementally. Accepted: it is the
  cost of server mode's low-memory purpose; the format stays standard OpenAI.
- **Truncated-body errors**: a mid-stream failure surfaces as a transport error,
  not a clean status. Accepted — the alternative is buffering to keep the status
  option, which defeats the memory goal.

## Alternatives considered
- **NDJSON / a new streaming endpoint**: rejected — the web API stays standard
  OpenAI; we stream the same JSON bytes.
- **Priority via a header instead of `input` cardinality**: a header works but
  is an addition; cardinality already separates query from bulk and matches
  kenn's calls. The header is kept only as an optional override.
- **Buffer-then-send the response**: simplest, but materializes all vectors
  server-side, defeating the low-memory purpose. Rejected.

## Build order
```
  1. Daemon worker = the shared scheduler over its resident context; classify by
     input cardinality (+ optional header); replace the FIFO mpsc.
  2. Stream-parse the request `input`; stream-write the response `data` per
     batch; usage last.
  3. Mid-stream failure → drop the connection.
```
