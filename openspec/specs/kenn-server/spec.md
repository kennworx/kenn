# kenn-server Specification

## Purpose
TBD - created by archiving change extract-kenn-server. Update Purpose after archive.
## Requirements
### Requirement: A long-lived per-user kenn server hosts capability modules

The system SHALL provide a `kenn server` subcommand that runs a
long-lived per-user HTTP service hosting capability modules. v1
hosts one module (the embeddings API); future capabilities
(agent-to-agent communication, shared user history aggregated
from hooks) SHALL plug into the same host as sibling modules
sharing one address, one PID, one state directory, and one
`/healthz`.

The subcommand SHALL accept three actions: `start`, `stop`, and
`status`.

#### Scenario: a single host serves multiple capabilities

- **WHEN** the server starts with multiple capability modules registered
- **THEN** every module's routes are reachable on the same `[server].addr`
- **AND** the server writes one PID file at the per-OS state path
- **AND** `GET /healthz` returns 200 once every module's startup completes

#### Scenario: v1 ships embeddings only

- **WHEN** `kenn server start` runs at v1
- **THEN** the embeddings module is registered and its routes are reachable
- **AND** no other capability routes are exposed

### Requirement: The server binds the resolved address from global config

The HTTP listener SHALL bind to the address resolved with the
precedence `KENN_SERVER_ADDR` env var > `[server].addr` in the
global config > built-in default `127.0.0.1:41873`. The same
resolved address SHALL be used by auto-spawn clients to probe.

When **multiple daemons start concurrently** (e.g. several MCP instances
cold-start and each auto-spawns), the bind is the arbiter: exactly **one**
daemon binds the resolved address and the rest fail with `EADDRINUSE`. A
daemon that loses the bind SHALL exit without writing a PID file and
without corrupting the winner's state. Auto-spawn **clients** treat a lost
bind race as non-fatal — they converge on the winner via `/healthz` (see
`embedding-producer`); operator-run `kenn server start` still surfaces the
conflict with a non-zero exit so a human sees it.

#### Scenario: default address

- **GIVEN** no `KENN_SERVER_ADDR` env var and no `[server].addr` in global config
- **WHEN** `kenn server start` runs
- **THEN** the server binds `127.0.0.1:41873`

#### Scenario: env var overrides config

- **GIVEN** `[server].addr = "127.0.0.1:41873"` in global config
- **AND** `KENN_SERVER_ADDR=127.0.0.1:9999` in the environment
- **WHEN** `kenn server start` runs
- **THEN** the server binds `127.0.0.1:9999`

#### Scenario: bind conflict surfaces cleanly (operator-run)

- **GIVEN** another process already holds the resolved address
- **WHEN** `kenn server start` runs interactively
- **THEN** the server exits non-zero with a message naming the address and `EADDRINUSE`
- **AND** does not write a PID file

#### Scenario: concurrent auto-spawn resolves to one daemon

- **GIVEN** two processes auto-spawn a daemon at the same resolved address simultaneously
- **WHEN** both attempt to bind
- **THEN** exactly one binds and becomes the live daemon (writes the PID file)
- **AND** the other fails to bind, exits, and does not write or overwrite the PID file
- **AND** both spawning clients reach the winner via `/healthz`

### Requirement: Global config lives at the per-OS standard config path

The system SHALL load global config from a per-OS standard path
resolved via the `directories` crate: `~/.config/kenn/kenn.toml`
on Linux/XDG, `~/Library/Application Support/kenn/kenn.toml` on
macOS, `%APPDATA%\kenn\kenn.toml` on Windows. The file is
optional — missing fields and a missing file both fall through
to built-in defaults.

This requirement owns the *file* (path, format, optionality,
precedence rules). Capability specs (`embeddings-api` and any
future capability) own the *semantics* of their own `[section]`
within the file.

The workspace-local `kenn.toml` SHALL NOT participate in global
configuration. Embedding and server settings are user-wide.

#### Scenario: missing global config file

- **WHEN** no global config file exists
- **THEN** all global settings take their built-in defaults
- **AND** the server starts normally

#### Scenario: workspace kenn.toml does not affect server settings

- **GIVEN** a workspace `kenn.toml` containing a `[server]` table
- **WHEN** `kenn server start` runs from that workspace
- **THEN** the workspace `[server]` table is ignored
- **AND** the server uses the global config (or defaults)

### Requirement: PID file at the per-OS state directory

The server SHALL write its PID atomically after `bind` succeeds
to a file at the per-OS state directory: `$XDG_STATE_HOME/kenn`
when set, otherwise `~/.local/state/kenn` on Unix — so the path
is `~/.local/state/kenn/server.pid` on **both Linux and macOS**
(macOS no longer uses `~/Library/Application Support/kenn/`), and
`%LOCALAPPDATA%\kenn\server.pid` on Windows. The file SHALL be
removed on graceful shutdown.

The PID file is the authoritative source for `kenn server stop`
and `kenn server status`. A stale PID file (no process with that
PID) SHALL be treated as "not running" and removed by the next
`start` or `status` invocation.

#### Scenario: stop uses HTTP-graceful shutdown as the primary path

- **GIVEN** a kenn server reachable at the configured URL (local OR externally-managed; PID-file presence is irrelevant)
- **WHEN** `kenn server stop` runs
- **THEN** the command POSTs `/admin/shutdown`, sees HTTP 202, polls `/healthz` until the listener closes (bounded by a 15 s grace), and prints `stopped (graceful)` with the URL
- **AND** the server's modules' `shutdown` hooks have run (e.g. embeddings released its model)
- **AND** the PID file (if any) has been removed by the server itself, not by `stop`

#### Scenario: stop falls back to PID-file SIGTERM when HTTP is unreachable

- **GIVEN** a hung local daemon — PID file exists, the process is alive, but `/admin/shutdown` does not respond within a 2 s timeout
- **WHEN** `kenn server stop` runs
- **THEN** the command falls back to reading the PID, sending SIGTERM → polling 5 s → SIGKILL → removing the PID file
- **AND** prints `stopped (via PID file, HTTP was unreachable)` with the URL

#### Scenario: status always reports the configured URL and probes /healthz

- **WHEN** `kenn server status` runs in any state
- **THEN** the output includes the resolved server URL (e.g. `http://127.0.0.1:41873`)
- **AND** `/healthz` is probed regardless of PID-file presence (truth is at the port — a daemon may be running externally with no local PID file, or a stale PID may sit next to an externally-bound one)

#### Scenario: running locally — PID file matches a healthy daemon

- **GIVEN** a PID file pointing at a live process AND `/healthz` returns 200 at the configured URL
- **WHEN** `kenn server status` runs
- **THEN** the output reports `running (pid N, healthy)` with the URL

#### Scenario: running externally — /healthz responds with no local PID file

- **GIVEN** no PID file (or one cleaned up as stale) AND `/healthz` returns 200 at the configured URL
- **WHEN** `kenn server status` runs
- **THEN** the output reports `running externally (responded to /healthz; no local PID file)` with the URL

#### Scenario: unresponsive — PID alive but /healthz does not respond

- **GIVEN** a PID file pointing at a live process AND `/healthz` does NOT respond
- **WHEN** `kenn server status` runs
- **THEN** the output reports `pid N alive but /healthz unreachable (unresponsive)` with the URL — surfacing the split state so the operator can investigate

#### Scenario: stale PID file is cleaned up by status

- **GIVEN** a PID file pointing at a PID with no live process AND `/healthz` does not respond
- **WHEN** `kenn server status` runs
- **THEN** the output reports `not running (stale PID file cleaned up)` with the URL, and the stale PID file is removed

#### Scenario: stale PID file is cleaned up by stop

- **GIVEN** a PID file pointing at a PID with no live process
- **WHEN** `kenn server stop` runs
- **THEN** the command exits 0, reports "not running", and removes the stale file
- **AND** does NOT signal any unrelated process that may have inherited the PID

### Requirement: Daemon-by-default with a foreground override

`kenn server start` SHALL daemonize by default — the parent
process spawns a detached child, polls `/healthz` against the
configured address until the listener answers (within a bounded
budget), prints the URL and a `started` confirmation, and
returns. The user's shell prompt SHALL return only AFTER the
daemon is confirmed-listening; this lets shell pipelines like
`kenn server start && curl …` work without an explicit
readiness wait.

A `--foreground` flag SHALL keep the process attached to the
invoking shell with no spawn, suitable for systemd / launchd
supervisors.

If the spawned daemon does not report healthy within the budget
(default 10 s), the parent SHALL exit non-zero with an error
message naming the URL and the log file path.

#### Scenario: daemon mode waits for readiness

- **WHEN** `kenn server start` runs without `--foreground`
- **THEN** the command spawns a detached child, polls `/healthz` at the configured URL, and returns with exit code 0 only after `/healthz` answers 200
- **AND** the output is `kenn server: http://addr — started`
- **AND** the PID file (written by the daemonized child) is in place by the time the parent returns
- **AND** `kenn server status` immediately after returns `running (pid N, healthy)`

#### Scenario: daemon fails to come up — parent exits non-zero

- **WHEN** the spawned daemon's `/healthz` does not answer within the budget (e.g. bind conflict, missing model dir permissions, etc.)
- **THEN** the parent exits non-zero with a message naming the configured URL and the path to `<state_dir>/server.log` for inspection

#### Scenario: foreground mode

- **WHEN** `kenn server start --foreground` runs
- **THEN** the process stays attached to the invoking shell
- **AND** SIGINT terminates the server gracefully

#### Scenario: SIGTERM triggers graceful shutdown

- **GIVEN** a running server with in-flight capability requests
- **WHEN** the process receives SIGTERM
- **THEN** the server stops accepting new connections
- **AND** in-flight requests complete (subject to a bounded grace window)
- **AND** the PID file is removed before the process exits

### Requirement: Auto-spawned daemons exit on aggregate idle

An auto-spawned daemon SHALL exit cleanly after a configured
idle timeout passes with no client requests on any capability
route. The auto-spawn path is identified by the
`--idle-timeout` flag passed to `kenn server start`. The
default auto-spawn idle timeout SHALL be 600 seconds (10 minutes).

When the server was started without `--idle-timeout` (i.e. by a
human or a supervisor), the idle exit SHALL be disabled.

The idle counter is reset by *any* request to any capability
route. `/healthz` probes do NOT reset the counter — otherwise a
status-polling agent would keep the daemon alive forever.

#### Scenario: auto-spawned daemon exits when idle

- **GIVEN** a server started with `--idle-timeout 600`
- **WHEN** 600 seconds elapse with no capability requests
- **THEN** the server exits cleanly, removes its PID file, and logs the idle-exit reason

#### Scenario: human-started daemon does not exit on idle

- **WHEN** `kenn server start` runs (no `--idle-timeout` flag)
- **THEN** the server runs indefinitely regardless of request volume

#### Scenario: healthz probes do not reset the idle counter

- **GIVEN** an auto-spawned daemon with idle timeout 60s
- **WHEN** `/healthz` is polled every 10 seconds for 60 seconds with no capability requests
- **THEN** the server exits at the 60-second mark

### Requirement: GET /healthz reports readiness and lifecycle status

The host SHALL expose `GET /healthz` returning HTTP 200 once
every registered capability module's startup has completed. The
response body SHALL include a `status` field with one of:

- `"running"` — steady state.
- `"shutting_down"` — a graceful-shutdown trigger has fired (OS
  signal, idle timeout, or `POST /admin/shutdown`) and in-flight
  requests are draining; the listener is still accepting
  `/healthz` and `/admin/shutdown` polls but capability routes
  return 503 (see "rejects new capability requests during
  shutdown" below).

It SHALL NOT require any capability-specific state (e.g. a model
load) — readiness means "the host is listening and all modules
are wired."

#### Scenario: ready

- **GIVEN** a running server with all modules wired
- **WHEN** a client issues `GET /healthz`
- **THEN** the response is HTTP 200 with `{ "status": "running", "uptime_seconds": <n> }`

#### Scenario: status flips to shutting_down during the drain window

- **GIVEN** a running server that has just had a graceful-shutdown trigger fire (e.g. `POST /admin/shutdown`)
- **WHEN** a client polls `GET /healthz` between the trigger and the listener closing
- **THEN** the response is HTTP 200 with `{ "status": "shutting_down", "uptime_seconds": <n> }`
- **AND** once axum closes the listener (in-flight drain complete), subsequent `/healthz` probes fail to connect

### Requirement: POST /admin/shutdown triggers HTTP-graceful shutdown

The host SHALL expose `POST /admin/shutdown` as the
HTTP-graceful stop path used by `kenn server stop`. The handler
SHALL:

- Flip the host status to `shutting_down` atomically.
- Return HTTP 202 Accepted with `{ "status": "shutting_down" }`.
- Notify the graceful-shutdown task so axum stops accepting new
  connections and begins draining in-flight requests.

After the drain completes, every registered module's `shutdown`
hook runs (so e.g. the embeddings module releases its model
weights), the PID file is removed, and the process exits.

In v1 there is NO auth on this route. On shared hosts use a
per-user `KENN_SERVER_ADDR` (design R4) so the route isn't
reachable cross-user.

#### Scenario: admin shutdown drains gracefully

- **GIVEN** a running server with no idle timeout
- **WHEN** a client issues `POST /admin/shutdown`
- **THEN** the response is HTTP 202 with `{ "status": "shutting_down" }`
- **AND** the server exits cleanly within a bounded grace window (no SIGTERM, no idle timeout fired)
- **AND** the PID file is removed before the process exits

### Requirement: New capability requests are rejected during shutdown

Once the host status is `shutting_down`, capability routes SHALL
return HTTP 503 with an OpenAI-shaped error body
(`{ "error": { "type": "service_unavailable", "code": "shutting_down", ... } }`)
instead of being silently half-served. Internal routes
(`/healthz`, `/admin/shutdown`) SHALL remain reachable so
observers can watch the drain and `stop` is idempotent.

In-flight requests (those whose handler already started) SHALL
continue to completion — axum's `with_graceful_shutdown` drains
them before the listener closes.

#### Scenario: capability route returns 503 once shutting down

- **GIVEN** a running server that has just had `POST /admin/shutdown` fire
- **WHEN** a NEW capability request arrives over an existing keep-alive connection
- **THEN** the response is HTTP 503 with `code: "shutting_down"`
- **AND** `/healthz` continues to respond 200 with `status: shutting_down` until the listener closes

### Requirement: The per-user daemon detaches without forking

When `kenn server start` runs in daemon mode, the server SHALL detach into its
own session using `setsid` only, without a fork-without-exec. The spawning
parent SHALL fork-exec the server as a fresh process and redirect its stdio to
the server log. The daemon's embedder (llama.cpp / Metal) SHALL be able to
create a compute context and serve `/v1/embeddings`.

#### Scenario: daemonized server serves embeddings

- **GIVEN** `kenn server start` invoked in daemon mode on macOS
- **WHEN** a client POSTs to `/v1/embeddings`
- **THEN** the server returns embedding vectors (not a 503 context-creation
  failure)
- **AND** `kenn embed` completes against the daemon

#### Scenario: daemon outlives the launching shell and logs

- **WHEN** the launching `kenn server start` returns
- **THEN** the daemon keeps running detached from the controlling terminal
- **AND** its stdout/stderr are written to `<state_dir>/server.log`

