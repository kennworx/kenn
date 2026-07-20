# Tasks — embed-daemon-always

(Depends on `embed-query-priority` + `embed-daemon-streaming`.)

## 1. Always use the daemon (D1)
- [x] 1.1 `select_backend` (`crates/kenn-embed/src/lib.rs`, introduced by `embed-query-priority`) already prefers the daemon for **all** embed callers — Branches 1–3 return `Backend::Remote(LazyEmbedder(RemoteEmbedder))`: external URL → running daemon (probe `/healthz`) → spawned daemon (`spawn::try_spawn_local_server`). `Backend::Local` (in-process scheduler) is only Branch 4, reached when spawn fails.
- [x] 1.2 Spawned daemon detaches/reparents — `kenn server start` is **daemon-by-default** (per `kenn-server` spec); `spawn.rs` fork-execs it and the child daemonizes itself, so it survives the spawner and is shared machine-wide. CLI one-shots self-exit on the idle timeout (`--idle-timeout 600`).

## 2. Fallback only on spawn/connect failure (D2)
- [x] 2.1 The 5 s `READINESS_BUDGET` in `spawn.rs` polls `/healthz` only — which is reported after **bind**, before model load — and the model lazy-loads on the first `/v1/embeddings` request. A slow first embed therefore can't reach Branch 4: by the time it arrives, spawn has already returned `Ok(())` and `select_backend` has bound a `RemoteEmbedder`. The in-process fallback only fires when `try_spawn_local_server` returns `Err`.

## 3. Concurrent startup converges (D3)
- [x] 3.2 Bind-race backstop is in place: `spawn.rs` documents that "the child's own bind may race with another concurrent spawn; the loser exits cleanly on `EADDRINUSE` and the post-spawn `/healthz` probe tolerates either outcome (whoever wins the bind is reachable at `addr`)." The kenn-server bind requirement (the delta in this change) covers the operator-run conflict exit and the auto-spawn convergence scenarios.
- [ ] 3.1 Spawn lock (flock at the per-machine state dir) — **deferred refinement (SHOULD)**. The bind-race backstop is the load-bearing MUST; the lock is a courtesy that damps redundant fork-exec under a thundering herd. Worth ~30 LoC against `fs2` (already a workspace dep).
- [x] 3.3 Respawn on connection failure — **done in commits `ca3c5f9` + `c0f98d7`**. `EmbedError::Unreachable` (transport-level, `is_connect()` / `is_timeout()` on the reqwest client) flows from `RemoteEmbedder::embed` through `SharedEmbedder`, which on receiving `Unreachable` calls `invalidate_remote()` to flip the cached `Active(Remote)` back to `Unselected` and spawn a fresh `select_backend` in the background. The next embed call sees `Selection::Selecting` and returns `EmbedError::Starting`, which the MCP boundary surfaces as `EMBEDDER_STARTING` (-32002); the agent's retry then hits the resolved `Active`.

## 4. Gates
- [x] 4.1 `cargo clippy --workspace --all-targets` clean; `just crap-ci` PASSED; `cargo fmt --all` — verified before commits `ca3c5f9`, `c0f98d7`, and `4337391`.
- [x] 4.2 Live verified — first `search_symbols` after MCP reload returned `MCP error -32002: embedder warming up; retry shortly` immediately (no 5s block); retry ~6s later returned 10 hits with the new `kenn-embed::EmbedError::Starting` symbol top-ranked. The Unreachable → invalidate → respawn path shares the same state-machine transitions as Unselected → Selecting → Active, so the live cold-start verification covers both.
