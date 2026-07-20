# embedding-producer Specification

## Purpose
TBD - created by archiving change embedding-producer. Update Purpose after archive.
## Requirements
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

### Requirement: corpus embeddings are produced only at index time and flush time

Embeddings for **stored content** — code symbols and findings — SHALL be generated only when the code-search index is built or when findings are flushed, never on the query path. Committed embeddings SHALL remain readable and searchable with no embedding model and no network present.

Embedding a free-text query string into a query vector is a distinct query-time operation, governed by the requirement below; it never generates or modifies a stored embedding.

#### Scenario: a fresh clone searches without a model

- **GIVEN** a repository whose committed stores contain embeddings
- **WHEN** it is cloned into an environment with no embedding model and no network, and a search is run
- **THEN** lexical search and item-to-item vector search (reusing committed vectors) return ranked results
- **AND** free-text vector search degrades to lexical-only rather than failing

### Requirement: free-text queries are embedded by a lazily-loaded query embedder

A free-text search query SHALL be turned into a query vector using the same embedding model as the corpus, so the query and stored vectors share one space. The query embedder SHALL be loaded on demand and released after an idle period — an idle search service SHALL hold no embedding model in memory.

#### Scenario: a free-text query loads the embedder on demand

- **WHEN** a free-text vector search is issued and no query embedder is resident
- **THEN** the embedder is loaded, the query string is embedded, and hybrid results are returned
- **AND** after an idle period with no further queries the embedder is released

#### Scenario: item-to-item search reuses a committed vector

- **WHEN** a "similar items" search uses an already-indexed item as its source
- **THEN** that item's committed embedding is reused as the query vector
- **AND** no embedding model is loaded

### Requirement: code rows and findings are embedded through the producer

On an index run, every code row that `lance-search` reconciliation marks for re-embedding SHALL be embedded via the producer and have its `embedding` column populated; unchanged rows SHALL reuse their committed embedding. On a findings flush, every newly committed finding SHALL be embedded via the producer.

#### Scenario: a changed symbol is re-embedded

- **WHEN** an index run reconciles a symbol whose `embeddable_text` fingerprint changed
- **THEN** that symbol is embedded by the producer and its `embedding` column is populated

#### Scenario: a flushed finding carries an embedding

- **WHEN** a pending finding is flushed to the committed store
- **THEN** its `embedding` column is populated by the producer

### Requirement: the vector index and hybrid search activate once embeddings exist

Once the `embedding` column is populated, the Lance native vector index SHALL be built over it for both the code-search and findings datasets, and search SHALL blend BM25 and vector similarity into one ranked result.

#### Scenario: retrieval by meaning

- **WHEN** a query paraphrases an indexed symbol or finding without sharing exact terms
- **THEN** hybrid search returns that symbol or finding among the ranked results

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

### Requirement: each producer implementation has a deterministic integration test against real weights

Each in-process implementation of the embedding-producer boundary SHALL be exercised by a dedicated integration test that loads its real model weights and runs the produce-vectors path end-to-end, asserting on structural properties of the output (vector count matches input count, vector dimension matches the producer's reported `dim()`, vectors are L2-normalized, vectors are not all-zero, distinct inputs produce distinct vectors). The integration test SHALL run deterministically when invoked — its execution MUST NOT depend on environment variables that silently skip the embed path, and its coverage MUST attribute to the producer crate directly rather than through transitive calls from unrelated test suites. The integration test MAY be opt-in (gated by `#[ignore]` or a build feature) and MAY be platform-gated to match the implementation's platform support; it SHALL be runnable via a documented developer-facing command (e.g. a justfile recipe) so anyone touching the implementation can verify the contract without consulting other docs.

#### Scenario: the in-process llama producer is exercised against real EmbeddingGemma weights on macOS

- **WHEN** a developer runs the documented kenn-embed integration test recipe on macOS with the EmbeddingGemma model available (cached or downloaded)
- **THEN** `LlamaEmbedder::load()` resolves the model and initializes the llama backend
- **AND** `LlamaEmbedder::embed(...)` returns one L2-normalized vector per input string
- **AND** every returned vector has the dimension reported by `LlamaEmbedder::dim()`
- **AND** vectors produced for distinct input strings are themselves distinct
- **AND** the test fails loudly (does not silently skip) when the model is unavailable

#### Scenario: producer integration coverage does not depend on indirect test paths

- **GIVEN** the embed-producer integration test exists
- **WHEN** the test runs under coverage instrumentation
- **THEN** coverage of the producer's `embed` and backend-init functions is attributed directly to the producer crate
- **AND** the producer's crap-gate status does not depend on whether a downstream test suite (e.g. `kenn-store::hybrid_search`) happened to load the model

### Requirement: Embedding requests carry an interactive-vs-bulk priority

The embedding-producer boundary SHALL distinguish **interactive** embeds
(a free-text query being vectorized for search) from **bulk** embeds
(background corpus embedding). This intent SHALL be carried from the call
site through the producer to the inference processor — it is not enough to
collapse a query into an ordinary batch embed, because the processor needs
the class to schedule it.

Interactive query embeds SHALL be classed **high** priority; background
bulk embeds **low**. When no class is supplied, a request defaults to
**bulk** (the safe default for the corpus pass).

#### Scenario: a query embed and a bulk embed are distinguishable at the producer

- **WHEN** a free-text query is embedded and, separately, a background corpus batch is embedded
- **THEN** the producer sees the query as high priority and the bulk batch as low priority
- **AND** a caller that supplies no class is treated as bulk

### Requirement: The inference worker batches at the model unit and serves queries ahead of bulk

The producer's inference worker SHALL run at most one model encode in
flight (the serialized-inference invariant) AND SHALL guarantee that an
interactive query embed is served ahead of pending bulk work, bounding the
query's wait to at most **one in-flight model batch** — never a whole bulk
request or pass.

The worker SHALL process inference in units of **one encode over at most
the model's internal batch size** (`SEQS_PER_BATCH`), reusing a single
resident model/context across batches rather than per-request. Inputs of
either priority are taken in batches of that size; the worker SHALL serve
all ready **high**-priority (interactive) batches before the next **low**
(bulk) batch. A batch SHALL NOT exceed `SEQS_PER_BATCH`: a large request is
processed as a sequence of such batches, and packing small same-class
requests together fills a batch only up to that ceiling. Each request's
results SHALL be reassembled in input order before it returns. This applies
to the **in-process** producer; the daemon worker is governed by
`embeddings-api` and SHALL be the same shared component.

A large bulk request therefore cannot occupy the worker beyond a single
model batch at a time; between batches a newly-arrived query batch is taken
first.

#### Scenario: a query embed preempts a large bulk request at the next batch

- **GIVEN** a large bulk embed request whose batches are queued/encoding in-process
- **WHEN** an interactive query embed arrives
- **THEN** the query is encoded after at most the one model batch currently in flight, not after the whole bulk request
- **AND** the bulk request still receives every one of its vectors, in input order

#### Scenario: one in-flight encode is preserved, context reused

- **WHEN** the worker is encoding any batch
- **THEN** no second encode runs concurrently
- **AND** priority takes effect at batch boundaries, not by interrupting an in-flight encode
- **AND** the resident model/context is reused across batches, not recreated per request

### Requirement: The embed path always uses the per-user daemon

The embedding producer selection SHALL always use the per-user daemon in the
steady state for **all** embed callers (MCP and CLI — `select_producer` is
shared; this is not MCP-only), rather than the in-process model — so server
mode's low-memory purpose (one model process; thin clients) holds. Selection
SHALL probe the daemon's `/healthz`; if up, use the remote producer; else
**spawn** the daemon and use the remote producer once healthy.

The spawned daemon SHALL **detach (daemonize) so it is reparented** away from
the spawning process — it outlives any single instance and is shared by all
MCP instances and CLI invocations. A CLI one-shot is fine: the auto-spawned
daemon carries an idle timeout and self-exits.

This does **not** by itself fix query-vs-bulk contention — it relocates
embedding into the daemon, where the priority scheduler (see
`embed-query-priority` / `embeddings-api`) resolves it.

#### Scenario: no running daemon — start one and use it

- **GIVEN** an embed is needed and no daemon is running
- **WHEN** the producer is resolved
- **THEN** it spawns the daemon (which daemonizes / reparents) and embeds via the daemon
- **AND** the daemon keeps running after the spawning process exits

### Requirement: In-process embedding is a last-resort fallback only

The in-process model SHALL be used **only** when the daemon cannot be spawned
or connected to, so embedding never hard-fails; it is not the normal path. The
fallback SHALL NOT be triggered by a **slow first embed**: the daemon reports
`/healthz` ready after it **binds**, while the model lazy-loads on the first
request (possibly a multi-minute first-ever model download). A slow first
request is therefore not a connect failure — falling back on it would load the
model twice and defeat the single-process goal. The spawn/probe waits on
`/healthz` (bind), not on model readiness.

#### Scenario: daemon unavailable — fall back in-process

- **WHEN** the daemon cannot be started or reached at all
- **THEN** the embed path falls back to the in-process model so embedding still works

#### Scenario: slow first embed does not trigger fallback

- **GIVEN** a freshly spawned daemon whose model is still loading on its first request
- **WHEN** the client awaits that first embed
- **THEN** it waits for the daemon rather than falling back in-process
- **AND** the model is not loaded a second time in the client process

### Requirement: Concurrent embed-path startups converge on one daemon

Concurrent embed-path startups SHALL converge on a **single** daemon when
multiple processes resolve their producer at once and each may spawn one.
Resolution is at the port bind: exactly one daemon binds the resolved address;
others fail to bind. A daemon **binds before** loading any model, so a loser
fails at bind and exits immediately, wasting no model load, and without writing
or overwriting the PID file. A client SHALL treat a lost bind race as
**non-fatal** — after spawning it re-probes `/healthz` and connects to whichever
daemon bound. A client SHOULD additionally take a per-machine spawn lock around
"probe → spawn → await healthz" to damp redundant spawns; the bind-race
tolerance is the backstop. A client SHALL treat a later connection failure (e.g.
the daemon idle-exited) as a reason to respawn.

#### Scenario: two cold-starting clients yield one shared daemon

- **GIVEN** two clients cold-start simultaneously with no daemon running
- **WHEN** both attempt to start the daemon
- **THEN** exactly one daemon binds the address and the other spawn loses the bind
- **AND** both clients connect to the daemon that bound (via `/healthz`)
- **AND** neither client errors due to losing the bind race

### Requirement: EmbeddingGemma queries are embedded with the model's query task prompt

When the producing model is EmbeddingGemma-family, the producer SHALL prepend
the model's query task-instruction prompt (`task: search result | query: `) to
**query-kind** embeds before tokenization. Document-kind embeds SHALL be sent
raw — the document prompt is deferred (measured as adding nothing over
query-only), so corpus embedding output is byte-identical to the unprompted
behavior and stored vectors need no invalidation.

The prompt SHALL be applied inside the producer boundary, keyed on the model id —
a producer configured for a **non-EmbeddingGemma** model SHALL send raw text with
no prompt for either kind. The prompt SHALL NOT be stored in `embeddable_text`;
only the bytes fed to the tokenizer carry it.

#### Scenario: query and document of the same text embed differently

- **GIVEN** an EmbeddingGemma producer
- **WHEN** the same string is embedded once as a query and once as a document
- **THEN** the two vectors differ (the query carried the task prompt; the
  document did not)

#### Scenario: document embedding is unchanged by this feature

- **GIVEN** an EmbeddingGemma producer
- **WHEN** a code symbol or finding is embedded as a document
- **THEN** the raw `embeddable_text` is tokenized with no prompt, producing the
  same vector as before the query prompt existed (existing indexes reuse their
  vectors with zero re-embeds)

#### Scenario: a non-EmbeddingGemma model receives no prompt

- **GIVEN** a producer configured for a non-EmbeddingGemma model id (e.g. a
  remote ollama model)
- **WHEN** any text is embedded as either kind
- **THEN** the raw text is sent with no task prompt prepended

### Requirement: the embed kind is explicit at the producer boundary

The embedding-producer boundary SHALL carry an explicit embed kind — query versus
document — distinct from scheduler priority. Corpus embedding SHALL use the
document kind and free-text query embedding SHALL use the query kind. Prompt
selection SHALL derive from this kind, not from the interactive-vs-bulk priority.

#### Scenario: corpus and query paths carry distinct kinds

- **WHEN** a code symbol is embedded at index time
- **THEN** it is embedded with the document kind
- **AND WHEN** a free-text query is embedded
- **THEN** it is embedded with the query kind

