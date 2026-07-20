## MODIFIED Requirements

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
