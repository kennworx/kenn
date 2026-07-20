# Design — always use the daemon; converge concurrent startups

Depends on `embed-query-priority` (scheduler) + `embed-daemon-streaming`
(daemon serves via it). This change governs **producer selection and daemon
lifecycle**, not the scheduler.

## Decisions

### D1. The embed path always uses the daemon (spawn + reparent if absent)
`select_producer` SHALL prefer the per-user daemon for **all** embed callers
(the policy is global — `select_producer` is shared by MCP and CLI, not
MCP-only):
1. probe `/healthz` at the resolved address — if up, use `RemoteEmbedder`;
2. else **spawn** `kenn server start` and use `RemoteEmbedder` once it is up.

The spawned daemon SHALL **detach / reparent** (it daemonizes — daemon-by-
default), so it is not a child tied to the spawning process and is **shared**
machine-wide. CLI one-shots are fine: the auto-spawned daemon carries an idle
timeout and self-exits.

**This decision does not by itself fix the original 10-min stall** — it
*relocates* embedding into the daemon. With both the query and the bulk pass now
hitting the daemon, the contention moves there, where the priority scheduler
(`embed-query-priority` / `embed-daemon-streaming`) resolves it. D1 + the
scheduler together are the durable fix; D1 is what makes the single model
process the governing path.

### D2. In-process fallback only on spawn/connect failure — never a slow first embed
The in-process model SHALL remain only as a **last-resort fallback**, used
solely when the daemon **cannot be spawned or connected to**. It SHALL NOT be
triggered by a slow first embed: the daemon reports `/healthz` ready after
**bind**, while the model **lazy-loads on the first request** (which may be a
multi-minute first-ever GGUF download). A slow first request is therefore not a
connect failure — falling back on it would load the model **twice** (daemon +
in-process) and defeat the single-process goal. The spawn/probe waits for
`/healthz` (bind), not for model readiness.

### D3. Concurrent startup converges on whichever daemon binds the port
Several processes can cold-start at once and each may spawn a daemon. The race
is resolved at the **port bind**: exactly one daemon binds the resolved address;
the others get `EADDRINUSE`.

- A daemon **binds before** loading any model (model is lazy), so a losing
  daemon fails at bind and exits immediately — **no wasted model load**. It
  exits without writing or overwriting the PID file. (Operator-run `kenn server
  start` still surfaces the conflict non-zero; an auto-spawned loser simply
  loses — its client does not depend on *its* spawn winning.)
- Every spawning client SHALL converge by **re-probing `/healthz`** after the
  spawn: whoever won is serving the address, so all clients attach to that one
  daemon. A lost bind race is **not** a client error.
- A client SHOULD take a **spawn lock** (flock on a lockfile at the resolved
  per-machine state dir) around "probe → spawn → await healthz" to damp a
  thundering herd; the bind-race tolerance is the backstop. The daemon is
  `127.0.0.1` (per-machine), so the lock and the bind race are both per-machine.

## Tradeoffs / risks
- **Idle-exit vs in-flight**: an auto-spawned daemon may hit its idle timeout
  and exit between a client's probe and request. The client SHALL treat a
  connection failure as "respawn" (loop back to D1), so a just-exited daemon is
  transparently restarted.
- **CLI one-shots spawn a daemon**: accepted — it self-exits on idle, and the
  shared model process is the point. A truly isolated run can still reach the
  in-process fallback if the daemon is unavailable.

## Build order
```
  1. select_producer: probe → spawn(detached) → use; in-process only on
     spawn/connect failure (D1/D2). Spawn waits on /healthz (bind), not model.
  2. Spawn lock around probe→spawn→await-healthz; daemon binds before model
     load; losing daemon exits without PID write; client re-probes and attaches
     to the winner (D3).
```
