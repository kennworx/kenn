## ADDED Requirements

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

### Requirement: Inference is single-worker with optional batch coalescing

The embeddings capability SHALL run inference through a single
worker — at most one llama.cpp call is in flight at any moment.
The worker MAY coalesce queued requests into one batched
inference call when multiple requests are waiting: each queued
request's `input` rows are concatenated into one llama.cpp
batch, the batched result is split back to each caller, and
each caller's response carries only its own vectors in its own
input order.

Coalescing SHALL preserve correctness: every caller receives
exactly the vectors for the inputs it sent, in the order it
sent them, and `usage.prompt_tokens` reflects only that
caller's inputs.

There is no priority queue in v1 — coalescing is FIFO at the
boundary between batches.

#### Scenario: two concurrent single-string requests both succeed

- **GIVEN** a running server with the model resident
- **WHEN** two `/v1/embeddings` requests arrive simultaneously, each with a single-string `input`
- **THEN** both succeed and return their respective vectors
- **AND** each response's `data` contains exactly one entry corresponding to that caller's input

#### Scenario: coalesced batch preserves per-caller accounting

- **GIVEN** two concurrent requests A (`input: "alpha"`) and B (`input: ["beta", "gamma"]`) coalesced into one inference call
- **WHEN** the worker fans the results back
- **THEN** A's response contains exactly one entry for "alpha"
- **AND** B's response contains exactly two entries in order ["beta", "gamma"]
- **AND** A's `usage.prompt_tokens` counts only "alpha"; B's counts only "beta" + "gamma"

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
