## MODIFIED Requirements

### Requirement: A remote producer over an OpenAI-compatible HTTP endpoint

The system SHALL provide a `RemoteEmbedder` implementation of
the producer boundary that calls a remote
OpenAI-compatible `/v1/embeddings` endpoint. It SHALL be usable
interchangeably with the in-process `LlamaEmbedder` from the
perspective of every storage and search call site.

The configured URL SHALL be the **base URL** of the provider
(e.g. `http://localhost:11434`); the client SHALL append
`/v1/embeddings` and `/v1/models` itself. A trailing slash on
the configured URL SHALL be tolerated. This matches the
convention used by ollama, lm-studio, and the OpenAI SDKs.

A single `embed(texts: &[&str])` call MAY translate to more than
one HTTP request. The producer SHALL split its input into chunks
of at most `EmbeddingsConfig::batch_size` strings (default 256)
and issue one `POST /v1/embeddings` per chunk, concatenating the
results in input order. This bounds per-request body size and
per-request latency so a large batch cannot exceed the client's
HTTP timeout; it is internal to the producer and transparent to
its callers. A `batch_size` of 0 SHALL fall back to the built-in
default.

When the remote endpoint is unreachable or returns an error,
the producer SHALL surface the failure such that the calling
`LazyEmbedder` degrades to `Ok(None)` — the same offline
degradation the in-process embedder already provides on load
failure. A failure on any chunk SHALL abort the whole `embed`
call (no partial results); the error class is preserved unchanged
(`Unreachable` vs `Backend`).

#### Scenario: remote producer embeds via HTTP

- **GIVEN** a `RemoteEmbedder` configured with base URL `http://host:port`
- **WHEN** a batch of text is embedded through it
- **THEN** it issues `POST http://host:port/v1/embeddings` with `{ model, input }`
- **AND** returns the parsed vectors in input order

#### Scenario: trailing slash on base URL is tolerated

- **GIVEN** a `RemoteEmbedder` configured with `http://host:port/`
- **WHEN** a batch of text is embedded through it
- **THEN** it issues `POST http://host:port/v1/embeddings` (no double slash)

#### Scenario: large batches split into multiple HTTP requests

- **GIVEN** a `RemoteEmbedder` configured with `batch_size = 256` and a call to `embed` with 600 input strings
- **WHEN** the producer executes the call
- **THEN** it issues exactly three `POST /v1/embeddings` requests with input arrays of size 256, 256, and 88
- **AND** the returned `Vec<Vec<f32>>` has 600 entries in the original input order
- **AND** no single HTTP request body carries more than `batch_size` inputs

#### Scenario: a chunk failure aborts the whole call

- **GIVEN** a `RemoteEmbedder` whose endpoint succeeds for the first request and returns HTTP 500 for the second
- **WHEN** an `embed` call requires three chunks
- **THEN** the call returns `Err(EmbedError::Backend(_))` after the second request
- **AND** no third request is issued
- **AND** no partial vectors are returned to the caller

#### Scenario: remote endpoint unreachable degrades to lexical-only

- **GIVEN** a `RemoteEmbedder` whose endpoint is not listening
- **WHEN** a free-text search runs
- **THEN** the embed call returns `Ok(None)` to the search layer
- **AND** the search degrades to lexical-only rather than failing

#### Scenario: remote returns non-2xx (model mismatch, server error)

- **GIVEN** a `RemoteEmbedder` whose endpoint responds with HTTP 404 (unknown model id), 500 (internal error), or any other non-2xx
- **WHEN** an embed call is issued
- **THEN** the producer surfaces the failure such that `LazyEmbedder` returns `Ok(None)` to the caller
- **AND** no vectors are produced, so no sidecar segment is written and the manifest is unchanged
- **AND** the producer is NOT swapped for `LlamaEmbedder` mid-process — the once-per-process selection stands
- **AND** subsequent calls in this process may succeed if the failure was transient (5xx, network blip); persistent failures (e.g. configured model id not served by the remote) will continue to return `Ok(None)` for this process's lifetime
