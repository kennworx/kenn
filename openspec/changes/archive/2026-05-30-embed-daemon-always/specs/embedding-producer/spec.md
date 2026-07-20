## ADDED Requirements

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
