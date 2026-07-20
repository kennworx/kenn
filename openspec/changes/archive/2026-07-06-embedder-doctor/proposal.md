## Why

kenn has **no way to ask "is embedding actually working, and if not, why?"** —
and its embedder is known-fragile on macOS (the daemonized `kenn server` embedder
hits a fork+Metal bug and returns empty embeddings; workaround is `--foreground`).
Today that failure is **invisible**:

- `kenn status` reads `meta.json` only — never touches the embedder.
- `kenn server status` / `/healthz` report daemon liveness + uptime, not embedding.
- MCP `get_index_status` maps to `ready`/`disabled`, but `disabled` means only
  "no model configured"; a *real* backend failure during the embed pass is
  **caught, logged at WARN, and then `EmbedStage::Ready` is set anyway**
  (`crates/kenn-mcp/src/indexing/orchestrate.rs:384`). So a fork+Metal 503 shows
  up as `ready` plus a stderr line no one sees — the agent believes embeddings
  exist and silently gets lexical-only results.

The exact cause is already in hand: `EmbedError::Backend(String)`
(`crates/kenn-embed/src/producer.rs`) carries the raw llama.cpp/Metal error text.
Nothing surfaces it.

## What Changes

- Add **`kenn doctor`**: a command that actively probes the embedder by embedding
  a trivial string through `shared_embedder().embed_query("hello")` and reports:
  - `Ok(Some(v))` → healthy: embedding dimension, measured latency, and which
    backend served it (in-process llama vs remote daemon vs external URL);
  - `Ok(None)` → disabled / lexical-only (no model configured);
  - `Err(Backend(msg))` → the one-line summary **and** the full raw error text.
  Its exit code distinguishes healthy / disabled / failed. Harness mirrors the
  existing `crates/kenn-cli/src/cmd_embed.rs` (tokio rt + `shared_embedder`).
- **Make degradation visible**: a genuine embed-pass backend failure SHALL be
  reported as a distinct `degraded` state (carrying the error), not silently
  collapsed to `ready`, in `get_index_status` and `kenn status`.
- Expose a minimal backend accessor on `SharedEmbedder` so the probe can name the
  active backend (`selector::Backend` is currently `pub(crate)` with no getter).

## Capabilities

### Added Capabilities

- `embedder-diagnostics`: an on-demand embedder self-check and a visible degraded
  state, so embedding failures are diagnosable rather than silent.

## Impact

- **Behavior:** `kenn doctor` is new and read-only. The degraded-state change makes
  a previously-hidden failure visible in status surfaces; it does not change what
  gets embedded, only how failure is reported. Aligns with the standing rule that
  the embed path surfaces a clear error rather than blocking or lying.
- **Diagnostics:** turns the fork+Metal 503 (and any backend error) from "silent
  lexical-only" into a one-command diagnosis with the raw cause.
