## Why

Server mode's purpose — one process holds the model, everyone else is a thin
client — only holds if processes actually **use** the daemon. Today
`select_producer` can fall through to an **in-process** model (the path behind
the original 10-minute query stall: the MCP process ran the bulk embed itself).
This change makes the embed path always prefer the daemon (starting it if
absent) and specifies how concurrent startups converge on one daemon.

**Depends on** `embed-query-priority` + `embed-daemon-streaming` (the daemon is
only worth always-using once it schedules by priority and streams). On its own
this change does **not** fix the stall — it *relocates* embedding into the
daemon, where the priority scheduler resolves query-vs-bulk contention; the two
together are the durable fix.

## What Changes

- **The embed path always uses the daemon.** `select_producer` SHALL probe the
  daemon's `/healthz`; if up, use the remote producer; else **spawn** the daemon
  and use it once healthy. This is the policy for **all** embed callers (MCP and
  CLI), not MCP-only — `select_producer` is shared. CLI one-shots are fine: the
  auto-spawned daemon self-exits on its idle timeout.
- **Spawned daemon detaches / reparents** (it daemonizes — daemon-by-default),
  so it is not a child of the spawning process: it outlives any single instance
  and is shared machine-wide.
- **In-process is a last-resort fallback only** — used solely when the daemon
  cannot be **spawned or connected to**. It SHALL NOT be triggered by a slow
  first embed: the daemon reports `/healthz` ready after **bind**, while the
  model lazy-loads on first request, so a slow first request is not a connect
  failure (avoids loading the model twice).
- **Concurrent startup converges on one daemon.** Exactly one daemon binds the
  port; losers fail to bind, exit without touching the PID file, and their
  clients re-probe `/healthz` and attach to the winner. A lost bind race is
  **non-fatal** for clients. A per-machine spawn lock damps redundant spawns.

## Capabilities

### Modified Capabilities
- `embedding-producer` (selection): the embed path always uses the daemon —
  probe → spawn (detached) if absent → use it; in-process only as last-resort
  fallback on spawn/connect failure. Concurrent startups converge on the one
  daemon that binds the port.
- `kenn-server`: the bind requirement covers **concurrent auto-spawn** — exactly
  one daemon binds, losers exit without writing/overwriting the PID file, and
  spawning clients converge on the winner via `/healthz`.

## Impact

- **Memory**: one model process machine-wide; MCP/CLI hold no model in the
  normal case.
- **Robustness**: simultaneous cold-starts resolve to a single shared daemon
  without errors.
- **Out of scope**: the scheduler (`embed-query-priority`) and the daemon's
  streaming protocol (`embed-daemon-streaming`).
