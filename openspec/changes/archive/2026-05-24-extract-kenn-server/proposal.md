## Why

Every `kenn mcp` process loads its own copy of the embedding
model (EmbeddingGemma-300M GGUF via `llama-cpp-2`, ~300 MB
resident plus Metal/CUDA context). N MCP attachments = N copies.

Near-term capabilities beyond embeddings also need *user-level*
(not workspace-level) state and coordination — inter-agent
communication, shared user history aggregated from hooks. All
want the same thing: a long-lived **per-user** kenn process that
short-lived workspace invocations talk to over HTTP.

This change extracts that daemon. v1 ships the host plus one
capability — OpenAI-compatible embeddings. Memory and
agent-comms are future modules plugging into the same host;
mentioned here only to explain the host's shape.

## What Changes

### Host

- **NEW** `kenn server` subcommand (`start | stop | status`).
  `start` runs HTTP on the resolved `[server].addr` (default
  `127.0.0.1:41873`). Daemon-by-default; `--foreground` opts
  out. PID file in the per-OS state dir via the `directories`
  crate.

- **NEW** `GET /healthz` — readiness probe, capability-agnostic.

- **Lifecycle.** Daemon outlives its spawner. Auto-spawned
  daemons exit after a process-idle timeout (default 10 min);
  externally-started daemons disable the timeout. Manual stop
  via `kenn server stop` (PID file → SIGTERM, grace, SIGKILL).

- **NEW** global config at the per-OS standard path
  (`~/.config/kenn/kenn.toml` on Linux, equivalents elsewhere):

    ```toml
    [server]
    addr = "127.0.0.1:41873"

    [embeddings]
    url = "http://localhost:11434"   # optional; unset → use this kenn server
    model = "embeddinggemma-300M"
    ```

  Workspace-local `kenn.toml` does NOT participate — embedding
  and server settings are user-wide.

- **NEW** env overrides: `KENN_SERVER_ADDR`, `KENN_EMBED_URL`,
  `KENN_EMBED_MODEL`. Precedence: env > global config > default.

- **Extensibility.** The server crate is a thin HTTP host with
  capability modules plugged in. Future capabilities are sibling
  modules sharing the same lifecycle, addr, PID, state dir, and
  `/healthz` — not separate daemons.

### First capability: OpenAI-compatible embeddings

- **NEW** `POST /v1/embeddings` and `GET /v1/models` on the kenn
  server. Both `encoding_format: "float"` and `"base64"` are
  supported; **`base64` is the default** — a deliberate deviation
  from OpenAI's `float` default. base64 carries the raw f32-LE
  bytes so the client reconstructs bit-identical f32 vectors
  (no f32 → JSON-number → f64 rounding) and the wire is ~3×
  smaller. Clients that want float arrays send `encoding_format:
  "float"` explicitly. Model loads lazily on first request,
  unloads on internal idle (the existing `LazyEmbedder` pattern,
  now inside the daemon).
  Concurrent single-string requests MAY be coalesced into one
  llama.cpp batch — under MCP fan-out the worker submits all
  queued requests as one batched inference, dramatically cutting
  per-request latency.

- **NEW** `RemoteEmbedder` in `kenn-store::embed` implementing
  the existing `EmbeddingProducer` trait over HTTP. The trait
  grows `fn identity(&self) -> String` returning the model id;
  the manifest stamps that string. The client always sends
  `encoding_format: "base64"` explicitly in the request body
  (kenn's server defaults to base64 anyway, but the explicit
  field makes it work uniformly against ollama / lm-studio /
  OpenAI, which default to `float`). The client transparently
  handles either encoding in the response.

- **MODIFIED** `shared_embedder()` becomes a two-branch selector
  on the resolved `embeddings.url`:
    - URL set → `RemoteEmbedder(url)`, no spawn, degrade on
      failure.
    - URL unset → probe `[server].addr`; if down, fork
      `kenn server start` and retry; if spawn fails, fall back
      to in-process `LlamaEmbedder`.

  `kenn index` (CLI batch path) also routes through
  `shared_embedder()` and gets the same sharing for free.

- **MODIFIED** sidecar manifest. The `[model]` table becomes
  `[embedding_model]` and records only the model id:

    ```toml
    [embedding_model]
    id = "embeddinggemma-300M"
    ```

  `gguf_xxh3` is removed. Provider URL is a runtime concern, not
  a property of the vectors — the same id served by ollama,
  lm-studio, or kenn's own server SHALL be treated as
  compatible. Runtime drift between providers for the same id is
  accepted as noise; operators who care should use distinct ids.
  Versioning lives in the id string the way OpenAI / ollama /
  lm-studio name models.

- **BREAKING** manifest. Old `[model]` sidecars are treated as
  incompatible and reconciliation re-embeds. No external
  consumers depend on the manifest yet.

## Impact

- **Specs**:
  - **NEW** `kenn-server` (host, lifecycle, config, state dir,
    `/healthz`).
  - **NEW** `embeddings-api` (`/v1/embeddings` + `/v1/models`).
  - **MODIFIED** `embedding-producer` (`identity()`,
    `RemoteEmbedder`, selector).
  - **MODIFIED** `incremental-embedding` (manifest stamps `id`
    only).

- **Code**:
  - New crate `kenn-server` (HTTP host + capability modules; v1
    wires only embeddings).
  - `kenn-store::embed`: new `remote.rs`, `identity()` on the
    trait, manifest schema change in `sidecar.rs`.
  - `kenn-config`: new `GlobalConfig` alongside workspace
    `Config`, per-OS path via `directories`.
  - `kenn-cli`: `server` subcommand + auto-spawn helper.

- **Out of scope (own changes later)**:
  - Agent-to-agent communication capability.
  - Shared user history / memory from hooks.
  - Multi-provider config section (named providers, per-provider
    model lists, auth, headers).
  - Auth (bearer tokens, mTLS).
  - Streaming / progress notifications.
  - Windows Service registration (v1 just detaches via
    `DETACHED_PROCESS`).
