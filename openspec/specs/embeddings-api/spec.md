# embeddings-api Specification

## Purpose
TBD - created by archiving change extract-kenn-server. Update Purpose after archive.
## Requirements
### Requirement: OpenAI-compatible embeddings route family

The kenn-server host SHALL expose an OpenAI-compatible
embeddings capability at `POST /v1/embeddings` and
`GET /v1/models`. The request and response shapes SHALL match
the OpenAI Embeddings API such that the same client code works
unchanged against kenn server, ollama, lm-studio, and hosted
OpenAI-compatible endpoints.

`POST /v1/embeddings` accepts a JSON body `{ "model": string,
"input": string | [string, ...], "encoding_format"?: "float" }`
and returns `{ "object": "list", "data": [{ "object":
"embedding", "index": int, "embedding": [float, ...] }, ...],
"model": string, "usage": { "prompt_tokens": int,
"total_tokens": int } }`.

`GET /v1/models` returns `{ "object": "list", "data": [...] }`
listing the model(s) the server can serve. v1 SHALL advertise
exactly one model: the id resolved from `[embeddings].model`
(env var > global config > default).

Both `encoding_format: "float"` and `"base64"` SHALL be
supported. When the field is absent, the server SHALL default to
**`"base64"`** — a deliberate deviation from the OpenAI default
of `"float"`. The motivation: base64 carries the raw f32-LE
bytes verbatim, so the client reconstructs bit-identical f32
vectors (no f32 → JSON-number → f64 rounding); it also yields
~3× smaller wire payloads. Clients that need float arrays MUST
request `encoding_format: "float"` explicitly.

For `"base64"`, the `embedding` field SHALL be a single string
holding the raw f32 little-endian bytes of the vector,
base64-encoded.

> The next two scenarios assert structure and accounting only —
> they apply regardless of whether the response carries `embedding`
> as a base64 string (the default) or a JSON array (with explicit
> `"encoding_format": "float"`). Encoding-specific assertions live
> in the dedicated scenarios further down.

#### Scenario: a single-string embed returns one vector

- **WHEN** a client posts `{ "model": "embeddinggemma-300M", "input": "hello" }` to `/v1/embeddings`
- **THEN** the response contains one entry in `data` with the embedding for "hello" and `index: 0`
- **AND** `usage.prompt_tokens` is at least 1 and equals the count returned by the model's tokenizer for "hello"
- **AND** `usage.total_tokens == usage.prompt_tokens`

#### Scenario: a batch embed returns vectors in input order

- **WHEN** a client posts `{ "model": "embeddinggemma-300M", "input": ["a", "bb", "ccc"] }`
- **THEN** the response contains three entries in `data` with `index` values 0, 1, 2 in input order
- **AND** `usage.prompt_tokens` equals the sum of the tokenizer's counts across all three inputs

#### Scenario: an empty input array is a 400

- **WHEN** a client posts `{ "model": "embeddinggemma-300M", "input": [] }`
- **THEN** the response is HTTP 400 with an OpenAI-shaped error body explaining that `input` must not be empty
- **AND** no model load is triggered if the model was not already resident

#### Scenario: unknown model id is a 404

- **WHEN** a client posts `{ "model": "does-not-exist", "input": "x" }`
- **THEN** the response is HTTP 404 with an OpenAI-shaped error body naming the unknown id and listing the available model id
- **AND** the server does NOT silently substitute its own configured model

#### Scenario: default encoding is base64

- **WHEN** a client posts a request omitting `encoding_format`
- **THEN** every entry's `embedding` field is a JSON string (base64-encoded raw f32-LE bytes), not a JSON array of floats

#### Scenario: explicit float encoding returns a JSON array

- **WHEN** a client posts `{ "model": "embeddinggemma-300M", "input": "hello", "encoding_format": "float" }`
- **THEN** every entry's `embedding` field is a JSON array of floats

#### Scenario: base64 and float decode to bit-identical f32 vectors

- **WHEN** the same input is embedded with `encoding_format: "float"` and `encoding_format: "base64"`
- **THEN** base64-decoding the second response's bytes as f32 little-endian, AND parsing the first response's JSON numbers as f32, yield two `Vec<f32>` of equal length with every component bit-identical (`a.to_bits() == b.to_bits()` for all dims)

#### Scenario: GET /v1/models lists exactly the configured model

- **WHEN** a client issues `GET /v1/models`
- **THEN** the response contains exactly one entry
- **AND** that entry's `id` equals the server's resolved `[embeddings].model` (default `embeddinggemma-300M`) with `owned_by: "kenn"`

### Requirement: The embedding model loads lazily and unloads when idle

The embeddings capability SHALL load the underlying llama.cpp
model on the first `/v1/embeddings` request, not at server
startup. Once loaded, the model SHALL be released after an
internal idle period (no embed calls) so an otherwise-active
server holding only `/healthz` traffic SHALL NOT hold an
embedding model in memory.

The internal idle TTL is independent of the host's process-idle
exit timeout: the model can unload and reload many times within
one daemon lifetime.

#### Scenario: first request loads the model

- **GIVEN** a freshly started server with no embed requests yet
- **WHEN** the first `/v1/embeddings` request arrives
- **THEN** the response delivers vectors
- **AND** subsequent requests reuse the resident model without reloading

#### Scenario: idle releases the model

- **GIVEN** a server that has served at least one embed request
- **WHEN** the configured internal idle TTL elapses with no further embed requests
- **THEN** the model is released and the process holds no embedding weights in memory
- **AND** a later embed request triggers a fresh load

### Requirement: Server-side configuration of the embeddings capability

The kenn-server host SHALL pass the `[embeddings]` section of
the resolved global config to the embeddings module at startup.
The embeddings module reads:

- `[embeddings].model` — the model id this server advertises via
  `/v1/models` and validates against in `POST /v1/embeddings`.
  Default: `embeddinggemma-300M`. Env-var override:
  `KENN_EMBED_MODEL`.

The `[embeddings].url` field is **not** read by the server — it
configures the *client-side* embedder selection in
`shared_embedder()` and SHALL be ignored by the kenn-server
process itself (a kenn-server pointed at another kenn-server is
not a supported v1 topology).

#### Scenario: configured model id flows into the advertised model

- **GIVEN** `[embeddings].model = "my-custom-id"` in global config
- **WHEN** `kenn server start` runs
- **THEN** `GET /v1/models` returns exactly one entry with `id == "my-custom-id"`
- **AND** `POST /v1/embeddings` accepts `model: "my-custom-id"` and rejects any other id with 404

#### Scenario: KENN_EMBED_MODEL overrides the config file at server start

- **GIVEN** `[embeddings].model = "from-config"` in global config
- **AND** `KENN_EMBED_MODEL=from-env` in the server's environment
- **WHEN** `kenn server start` runs
- **THEN** `GET /v1/models` returns exactly one entry with `id == "from-env"`
- **AND** `POST /v1/embeddings` accepts `model: "from-env"` and rejects `model: "from-config"` with 404

#### Scenario: KENN_EMBED_URL is ignored by the server process

- **GIVEN** `KENN_EMBED_URL=http://localhost:11434` in the server's environment
- **WHEN** `kenn server start` runs
- **THEN** the server starts normally serving its locally-loaded model
- **AND** does not relay any request to the URL in the env var

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

