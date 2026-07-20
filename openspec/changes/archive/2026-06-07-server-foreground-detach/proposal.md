## Why

`kenn server start` daemonizes by double-fork (the `daemonize` crate). On macOS
the resulting daemon's embedder fails **every** `/v1/embeddings` request with
`create embedding context: null reference from llama.cpp` (HTTP 503) —
**llama.cpp + Metal cannot create a compute context in a process that forked
without a following `exec`** (the `__THE_PROCESS_HAS_FORKED__…` guard). `/healthz`
still passes, so the embedder selector routes bulk embedding to the broken
daemon and `kenn embed` then errors out.

The in-process `LlamaEmbedder` (no fork) works fine — which is why `just
embed-smoke` and the fusion-spike harness embed correctly while the daemon path
fails. The bug blocks `kenn embed` / `kenn update` on macOS whenever they use
the auto-spawned server (the default path), affecting every workspace.

## What Changes

- The detach step (`kenn_server::runtime::daemonize`) becomes **`setsid` only —
  no fork**. The parent `kenn server start` already fork-*exec*'s a fresh child;
  that exec'd process is a clean address space where Metal initializes
  correctly, so it only needs session detachment, not a second fork.
- The spawning parent points the child's stdio at `<state_dir>/server.log` so
  the setsid-only daemon still logs.
- The `daemonize` crate dependency is removed (`nix::unistd::setsid` replaces it).

## Capabilities

### Modified Capabilities

- `kenn-server`: the per-user daemon detaches via `setsid` without forking, so
  its Metal-backed embedder works.

## Impact

- **Fixes:** `kenn embed` / `kenn update` and any `/v1/embeddings` use through
  the auto-spawned daemon on macOS.
- **Code:** `crates/kenn-server/src/runtime.rs` (daemonize),
  `crates/kenn-cli/src/cmd_server.rs` (child stdio → server.log),
  `crates/kenn-server/Cargo.toml` (drop `daemonize`).
- **Status:** implemented and verified — a daemon-mode `kenn server start` now
  returns real embeddings, and C2 was embedded end-to-end through it.
