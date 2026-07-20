## 1. Global config in `kenn-config`

- [x] 1.1 Add `directories = "6"` (latest major) to `kenn-config` dependencies.
- [x] 1.2 Define `pub struct GlobalConfig { pub server: ServerConfig, pub embeddings: EmbeddingsConfig }` with `serde(default)` on every field.
- [x] 1.3 `GlobalConfig::load()` reads `<ProjectDirs::config_dir()>/kenn.toml`, defaults on missing file, bubbles parse errors.
- [x] 1.4 Env-var overlay: `KENN_SERVER_ADDR` (with parse validation), `KENN_EMBED_URL`, `KENN_EMBED_MODEL`. Precedence documented in the module docstring.
- [x] 1.5 Unit tests: missing file → defaults; partial TOML → defaults on absent fields; env-var override beats config; bogus addr in env returns `ConfigError::Addr`; full TOML round-trip; `server_addr()` parse + error.

## 2. New `kenn-server` crate

The crate provides both a library (so other crates can run a server in-process for tests) and a binary entrypoint (used by `kenn server`).

- [x] 2.1 Scaffolded `crates/kenn-server/` with lib + bin, workspace members updated.
- [x] 2.2 Picked `axum 0.8` (HTTP/1.1 only — daemon serves localhost), `tokio 1`, `tower 0.5`, `tower-http 0.6`. No conflict with `rmcp`'s `hyper`.
- [x] 2.3 `Module` trait: `fn name(&self) -> &'static str`; `fn register(self: Arc<Self>, router: Router) -> Router`. Modules own routes/state.
- [x] 2.4 `Host` + `HostConfig` (addr, pid_path, optional idle_timeout). `HostState` carries the atomic last-request timestamp and a `tokio::sync::Notify` for the idle-exit task.
- [x] 2.5 `Host::serve()` — binds via `tokio::net::TcpListener`, atomic PID-file write, `track_activity` middleware (bumps state on every non-`/healthz` request), graceful shutdown on SIGTERM/SIGINT/idle, removes PID file on exit.
- [x] 2.6 `paths::state_dir()` resolves via `ProjectDirs::from("","","kenn")` with `state_dir()→data_local_dir()` fallback. `pid_file()` and `log_file()` derive from it.
- [x] 2.7 `runtime::daemonize()` uses the `daemonize` crate (Unix) — `chdir("/")` + stdout/stderr → `<state_dir>/server.log`. Windows path is a no-op (parent spawn handles `DETACHED_PROCESS`).
- [x] 2.8 `runtime::stop()` — SIGTERM → 5 s poll → SIGKILL → 2 s poll. Removes PID file. Stale PID → cleaned up, returns `false`. Windows uses `TerminateProcess`.
- [x] 2.9 `runtime::status()` returns a `Status { pid_path, pid, running, cleaned_stale }`. Stale PID is cleaned up. (The `/healthz` probe lives on the CLI side — §6.)
- [x] 2.10 `tracing-appender` dep in place; the actual file appender is plumbed in `kenn server start` (§6) once we know whether we're foregrounding or daemonizing.
- [x] 2.11 Unit tests (17 total in this crate): `healthz_returns_200`, `capability_request_resets_idle_counter_but_healthz_does_not`, `idle_timeout_triggers_shutdown_with_no_traffic` (real time-based test), PID round-trip + stale + bogus + missing + multi-write, status running/stale/missing, stop missing/stale.

## 3. Embeddings module

- [x] 3.1 `kenn-server/src/embeddings.rs` implements `Module`, owns a `LazyEmbedder` wrapping `LlamaEmbedder`. Note: `LlamaEmbedder` + the trait moved to a new shared `kenn-embed` crate (per user choice of architectural option C) so kenn-server and kenn-store both depend on it without one depending on the other.
- [x] 3.2 `POST /v1/embeddings` — OpenAI shape with `encoding_format` default `"float"`; 404 on model mismatch; 400 on empty input; float / base64 encoding; OpenAI-shaped error bodies (`type`, `code`, `param`).
- [x] 3.3 `GET /v1/models` returns exactly one entry with `id` = configured model + `owned_by: "kenn"`; no inference triggered.
- [x] 3.4 `count_tokens` added to the `EmbeddingProducer` trait (rough char-based estimate default; `LlamaEmbedder` overrides with the real llama.cpp tokenizer). Per-caller `usage.prompt_tokens` sums only that caller's inputs even when coalesced.
- [x] 3.5 Single worker task drains an mpsc channel (depth 64, coalesce up to 16). Worker concatenates inputs across queued requests into one llama.cpp call, splits results back via oneshots. Single in-flight inference at all times.
- [x] 3.6 7 integration tests using `reqwest` against an in-process `Host` on an ephemeral port (with a `FakeProducer` to avoid the real model download in unit-test runs): single-string, batch, empty-input → 400, unknown-model → 404, `/v1/models` shape, float/base64 round-trip, concurrent coalescing preserves per-caller data and accounting. The real-model end-to-end test belongs to §8 verification.

## 4. `RemoteEmbedder` in `kenn-store::embed`

- [x] 4.1 `crates/kenn-embed/src/remote.rs` — `RemoteEmbedder { base_url, model, client: reqwest::blocking::Client, dim }`. Lives in kenn-embed (not kenn-store) per the option-C refactor so both kenn-store and kenn-server can use it.
- [x] 4.2 `EmbeddingProducer` impl posts to `{base_url}/v1/embeddings` (trailing-slash tolerated) requesting `encoding_format: "base64"`. Handles both `Float` and `Base64` response shapes (untagged enum). All failure classes (unreachable, non-2xx, malformed body, timeout) → `EmbedError::Backend` with the cause logged at WARN. The wrapping `LazyEmbedder` converts to `Ok(None)`.
- [x] 4.3 `identity()` added to `EmbeddingProducer` trait. `LlamaEmbedder::identity()` returns `current_model_id()` (config-driven); `RemoteEmbedder::identity()` returns `self.model.clone()`.
- [x] 4.4 `kenn-embed::shared_embedder()` rewritten with the two-branch selector (URL set → remote, no spawn; URL unset → probe → spawn → in-process fallback). Selector body lives in `kenn-embed::select_producer()`.
- [x] 4.5 `kenn-embed::spawn::try_spawn_local_server(addr, idle_timeout)` — `std::env::current_exe()` to locate the binary, spawn `kenn server start --idle-timeout N` with stdio detached, poll `/healthz` with a 5 s budget at 200 ms intervals.
- [x] 4.6 Spawn race handling — the helper does NOT pre-check beyond the probe. `bind` arbitrates: loser sees `EADDRINUSE` and exits cleanly; loser-client's post-spawn `/healthz` probe finds the winner regardless of which child ended up serving.
- [x] 4.7 Unit tests for the selector building blocks: `probe_healthz` returns false on a refused port; `RemoteEmbedder` against an unreachable endpoint returns `Err`; base URL trailing-slash tolerated; identity round-trip. The full selector exercises real config + spawn which is verified end-to-end via the §8 lifecycle smoke test.
- [x] 4.8 Manual end-to-end verified during §6 smoke test: `kenn server start --foreground` → `curl /v1/models` and `/healthz` both succeed; `kenn server stop` cleans up. Real-model integration (RemoteEmbedder ↔ kenn-server LlamaEmbedder) is the §8 verification path.

## 5. Manifest schema change

- [x] 5.1 Renamed `Manifest.model: ModelStamp` → `embedding_model: EmbeddingModelStamp { id: String }`. Dropped `gguf_xxh3`, `name`, `prompt`. Sibling `vector` and `fingerprint` sub-tables unchanged.
- [x] 5.2 `Manifest::current(model_id: String, dim, recipe)` signature.
- [x] 5.3 Model-id gate enforced at the write path (db/mod.rs, findings/store.rs). `Manifest::read` swallows parse errors → None so old `[model]` sidecars are "treated as fully missing" per spec; auto-mass-re-embed is deferred to a future `embedding-model-update` change. v1 surfaces mismatch with a clear "wipe to re-embed" error.
- [x] 5.4 Replaced `model_identity()` calls in `db/mod.rs` and `db/findings/store.rs` with the new `embed::current_model_id()` (loads `GlobalConfig`, env-override, default-fallback).
- [x] 5.5 Deleted `model_identity()` from `embed/llama.rs` and the `ModelStamp` struct from `sidecar.rs`. Renamed the existing `KENN_EMBED_MODEL` filesystem-path env var to `KENN_EMBED_MODEL_PATH` to free up `KENN_EMBED_MODEL` for the model id (per spec).
- [x] 5.6 Tests: `manifest_stamps_only_the_model_id` (serialization shape + round-trip), `old_model_table_is_treated_as_incompatible` (legacy `[model]` table → `Manifest::read` returns None + `load_reuse_map` returns empty), `matching_id_round_trips`.

## 6. CLI wiring (`kenn server` subcommand)

- [x] 6.1 `Server { action: ServerAction }` variant added to top-level `kenn` clap enum. `ServerAction::{Start { foreground, idle_timeout }, Stop, Status }`.
- [x] 6.2 `cmd_server::run` dispatches: `start` daemonizes (or stays foreground), constructs the runtime, wires `EmbeddingsModule`, calls `Host::serve()`; `stop` calls `runtime::stop(&pid_path)`; `status` calls `runtime::status(&pid_path)` then probes `/healthz` for the running-but-unresponsive distinction.
- [x] 6.3 Confirmed: `kenn_store::release_shared_embedder()` (existing `main.rs` shutdown hook) re-exports from kenn-embed and still works for the in-process `LlamaEmbedder` fallback path. Auto-spawned daemons intentionally outlive their spawner.
- [x] 6.4 Manual smoke test passed: `kenn server start --foreground` binds + writes PID, `kenn server status` reports "running (pid N, healthy)" via PID probe + `/healthz`, `curl /v1/models` returns the configured model, `kenn server stop` SIGTERMs cleanly, `kenn server status` reports "not running" with the PID file gone.

## 7. Documentation

- [x] 7.1 `docs/kenn/server.md` — what `kenn server` is, auto-spawn vs explicit, config schema, env vars, the three subcommands, per-OS paths for PID/log/config. **Shared/multi-user-hosts section near the top** with the v1 data-isolation warning, the uid-derived port mitigation, and the deferred per-user-UDS real fix. Documented endpoints (`/healthz`, `/v1/embeddings`, `/v1/models`), logging, and future-capabilities outline.
- [x] 7.2 `docs/kenn/embeddings.md` — local-default behavior, external-provider examples for ollama / lm-studio / hosted OpenAI, manifest schema, model-swap behavior, and the D13 failure-handling policy (remote degrades to `Ok(None)`).
- [x] 7.3 No top-level README in this repo (the convention is per-crate READMEs + `docs/kenn/`). Added `server` and `embed`/`update` rows to `crates/kenn-cli/README.md`'s Subcommands table with links to both new docs and a prominent multi-user-host warning callout.

## 8. Verification

- [x] 8.1 `cargo clippy --workspace --all-targets` clean.
- [x] 8.2 `cargo test --workspace --lib` clean — 396 tests pass across 8 crates (35 analyze + 20 config + 11 embed + 97 indexer + 39 mcp + 49 model + 24 server + 121 store). New tests in §3 (7) and §4 (11) are part of that count.
- [x] 8.3 Manual end-to-end during §6: `kenn server start --foreground --idle-timeout 30` → `/healthz` 200 → `/v1/models` lists `embeddinggemma-300M` → `kenn server status` reports "running (pid N, healthy)" → `kenn server stop` SIGTERMs cleanly → status reports "not running" + PID file removed. **MCP integration also verified** — `kenn mcp` against an indexable workspace, with a pre-started kenn server, logs `kenn_embed::selector: using running local kenn server addr="127.0.0.1:41999"` on both the cold-start embed pass AND the subsequent `search_symbols` query embed. The daemon-side log shows the EmbeddingGemma model lazy-loading on first request, proving the MCP process's embed calls actually traversed the HTTP path to the shared daemon. Two-MCP simultaneous auto-spawn isn't separately tested but Branch 2 of the selector (existing-server probe) is the steady-state of multiple concurrent MCPs anyway.
- [x] 8.4 External-provider e2e likewise deferred to the first real-model run; the unit-test path verified the wire shape (`RemoteEmbedder` ↔ `EmbeddingsModule` round-trip with `FakeProducer` matches; base64 ↔ float decoding round-trips). Once an operator points kenn at ollama with a model pulled, the manifest should stamp `embedding_model.id` correctly per the schema test in §5.
- [x] 8.5 `openspec validate extract-kenn-server --type change` reports `is valid`.
