## MODIFIED Requirements

### Requirement: a pluggable embedding producer turns text into vectors

The system SHALL define an embedding-producer boundary — a single
interface that turns a batch of text into fixed-dimension float
vectors, exposes that dimension, and exposes the producing
model's identity string. All embedding generation SHALL go
through this boundary, so the underlying model is swappable
without changes to storage or search code.

The identity string SHALL be the model id (e.g.
`"embeddinggemma-300M"`) — not a content hash, not a provider
URL. The sidecar manifest gates vector reuse on this string
alone.

#### Scenario: text is embedded through the boundary

- **WHEN** a batch of text is passed to the producer
- **THEN** it returns one fixed-dimension vector per input
- **AND** every vector has the dimension the boundary reports

#### Scenario: producer reports its identity for the manifest

- **WHEN** a producer is queried for its identity
- **THEN** it returns the model id string that the sidecar manifest will record
- **AND** two producers configured for the same model id return the same string regardless of which transport (in-process llama.cpp, HTTP to kenn server, HTTP to ollama, HTTP to lm-studio) backs them

## ADDED Requirements

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

When the remote endpoint is unreachable or returns an error,
the producer SHALL surface the failure such that the calling
`LazyEmbedder` degrades to `Ok(None)` — the same offline
degradation the in-process embedder already provides on load
failure.

#### Scenario: remote producer embeds via HTTP

- **GIVEN** a `RemoteEmbedder` configured with base URL `http://host:port`
- **WHEN** a batch of text is embedded through it
- **THEN** it issues `POST http://host:port/v1/embeddings` with `{ model, input }`
- **AND** returns the parsed vectors in input order

#### Scenario: trailing slash on base URL is tolerated

- **GIVEN** a `RemoteEmbedder` configured with `http://host:port/`
- **WHEN** a batch of text is embedded through it
- **THEN** it issues `POST http://host:port/v1/embeddings` (no double slash)

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

### Requirement: A single selector chooses local versus remote at process startup

The process-global `shared_embedder()` SHALL select between the
remote and in-process producers from one resolved configuration:

1. If `embeddings.url` is configured (env var > global config), the
   selector SHALL use `RemoteEmbedder` against that URL and SHALL
   NOT auto-spawn a kenn server.
2. Otherwise the selector SHALL probe the resolved kenn-server
   `[server].addr`. If a server responds, the selector SHALL use
   `RemoteEmbedder` against it.
3. If no server responds, the selector SHALL attempt to fork-exec
   `kenn server start --idle-timeout N` (for some seconds value)
   and re-probe. On success, the selector SHALL use
   `RemoteEmbedder` against the freshly spawned server.
4. If the spawn fails (no executable in PATH, no permission, etc.),
   the selector SHALL fall back to the in-process `LlamaEmbedder`.

The selection happens once per process at first embed call;
subsequent calls reuse the chosen producer for the process
lifetime.

#### Scenario: explicit URL skips spawning

- **GIVEN** `KENN_EMBED_URL=http://localhost:11434`
- **WHEN** the first embed call happens
- **THEN** the selector chooses `RemoteEmbedder("http://localhost:11434")`
- **AND** no kenn server is forked even if no local kenn server is running

#### Scenario: no URL, no running server, auto-spawn succeeds

- **GIVEN** no `embeddings.url` configured
- **AND** no process listening at the resolved `[server].addr`
- **AND** `kenn server start` is on PATH and runnable
- **WHEN** the first embed call happens
- **THEN** the selector forks `kenn server start --idle-timeout 600`
- **AND** after the spawned server reports `/healthz`, the selector chooses `RemoteEmbedder` against the resolved addr

#### Scenario: no URL, no running server, spawn fails

- **GIVEN** no `embeddings.url` configured
- **AND** `kenn server start` cannot be executed (missing binary, denied execve, etc.)
- **WHEN** the first embed call happens
- **THEN** the selector falls back to `LlamaEmbedder` (in-process)
- **AND** subsequent embed calls within this process use the in-process embedder

#### Scenario: spawn succeeded but server never reports healthy in time

- **GIVEN** no `embeddings.url` configured
- **AND** the auto-spawn helper successfully forked a `kenn server start` child
- **WHEN** the helper's `/healthz` probe budget elapses without a successful response
- **THEN** the selector falls back to `LlamaEmbedder` (in-process) for this process
- **AND** the spawned child is left running — it may yet become healthy and serve later processes

#### Scenario: concurrent auto-spawn race resolves by bind

- **GIVEN** two processes simultaneously trigger auto-spawn after both probe-fail
- **WHEN** both spawned daemons attempt to `bind` the resolved addr
- **THEN** exactly one succeeds; the other exits cleanly with `EADDRINUSE`
- **AND** the loser-client's next probe finds the winner and connects
