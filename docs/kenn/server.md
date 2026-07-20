# kenn server

A long-lived **per-user** kenn daemon that workspace-local kenn
invocations (`kenn mcp`, `kenn index`, `kenn search`) talk to over
loopback HTTP. v1 hosts one capability — OpenAI-compatible
embeddings — so N MCP attachments share one resident model
instead of each loading its own EmbeddingGemma copy (~300 MB
resident + Metal/CUDA context per process).

Future capabilities (agent-to-agent communication, shared user
history aggregated from hooks) will plug into the same host as
sibling modules sharing the same lifecycle, address, PID file,
and `/healthz`.

## Shared / multi-user hosts — read this first

The v1 default binds `127.0.0.1:41873` with no auth. On a
multi-user machine where two users both have `kenn` installed:

1. User A's MCP auto-spawns a daemon; it binds the port.
2. User B's MCP probes the port — **User A's daemon responds**.
3. User B's selector chooses `RemoteEmbedder` against the probed
   address. No spawn, no `bind`, no `EADDRINUSE`.
4. User B's embed traffic (source code, doc text, finding
   contents) flows through User A's daemon process.

This is a **data-isolation hole**, not just a sharing
degradation. On a shared box, every user **MUST** set a unique
port:

```sh
# in each user's shell rc
export KENN_SERVER_ADDR="127.0.0.1:$((42000 + UID % 1000))"
```

…or in `~/.config/kenn/kenn.toml`:

```toml
[server]
addr = "127.0.0.1:42423"   # pick any free port; UID-derived
                           # avoids collisions in shared setups
```

A real fix (per-user UDS, uid-derived port, `/healthz` uid check)
is deferred until shared-host use is observed in practice.

## Subcommand: `kenn server`

```
kenn server start [--foreground] [--idle-timeout SECS]
kenn server stop
kenn server status
```

### `kenn server start`

- **Daemon mode** (default): the process daemonizes — the parent
  exits after the listener is bound, the child runs detached.
  On Unix uses the `daemonize` crate (double-fork + setsid +
  `chdir("/")` + log redirect). Windows uses the equivalent
  Win32 `DETACHED_PROCESS` detach.
- **`--foreground`**: keeps the process attached to the invoking
  shell, suitable for systemd / launchd supervisors.
- **`--idle-timeout SECS`**: when present, the daemon exits after
  SECS of no requests on any capability route (`/healthz` polls
  do NOT reset the counter). Auto-spawn from MCP / index passes
  `--idle-timeout 600`; human invocations normally don't (the
  daemon runs indefinitely).

### `kenn server stop`

Reads `~/.../server.pid` (per-OS state dir; see Paths below),
sends SIGTERM, polls for exit with a 5-second grace, then
SIGKILL if still alive. Removes the PID file. A stale PID file
(file present but the process is dead) is cleaned up with no
signal sent.

### `kenn server status`

Reports PID + a `/healthz` probe, distinguishing
running-and-healthy from running-but-unresponsive. Stale PID
files are cleaned up.

## Configuration

Global config lives at the per-OS standard config path:

| OS | Path |
|---|---|
| Linux/XDG | `~/.config/kenn/kenn.toml` |
| macOS | `~/Library/Application Support/kenn/kenn.toml` |
| Windows | `%APPDATA%\kenn\kenn.toml` |

```toml
[server]
addr = "127.0.0.1:41873"

[embeddings]
url = "http://localhost:11434"   # optional; unset → use this kenn server
model = "embeddinggemma-300M"
```

The workspace-local `kenn.toml` does **not** participate in
global configuration. Embedding and server settings are
user-wide.

### Env-var overrides

Precedence on every field: env var > global config > built-in
default.

| Var | Overrides | Notes |
|---|---|---|
| `KENN_SERVER_ADDR` | `[server].addr` | Validated at load. |
| `KENN_EMBED_URL` | `[embeddings].url` | Empty string treated as unset. |
| `KENN_EMBED_MODEL` | `[embeddings].model` | Model id (not a file path — see `KENN_EMBED_MODEL_PATH`). |
| `KENN_EMBED_MODEL_PATH` | local GGUF override | Filesystem path to a `.gguf` weights file. Bypasses the cache + download. |

## Paths

| Concern | OS | Path |
|---|---|---|
| Config | Linux | `~/.config/kenn/kenn.toml` |
| | macOS | `~/Library/Application Support/kenn/kenn.toml` |
| | Windows | `%APPDATA%\kenn\kenn.toml` |
| PID file | Linux | `~/.local/state/kenn/server.pid` |
| | macOS | `~/Library/Application Support/kenn/server.pid` |
| | Windows | `%LOCALAPPDATA%\kenn\server.pid` |
| Log file | (same as PID dir) | `<state_dir>/server.log` |
| GGUF cache | Linux | `~/.cache/kenn/models/` (or `$XDG_CACHE_HOME/kenn/models/`) |
| | macOS | same — XDG semantics |
| | Windows | same |

## Lifecycle

The daemon **outlives its spawner** — an MCP that auto-spawned
the daemon can die without killing it. Cleanup paths:

- **Auto-spawned daemons** exit after the `--idle-timeout`
  window passes with no requests on any capability route.
- **Externally-started daemons** (no `--idle-timeout`) run
  indefinitely until `kenn server stop` or the OS signals them.

## When does a daemon get spawned?

The embedding selector picks at the first embed call in any kenn
process (MCP, index, search):

1. `KENN_EMBED_URL` (or `[embeddings].url`) set → use that URL,
   no spawn ever. If the URL is unreachable, embedding silently
   degrades to lexical-only — the operator chose the URL,
   the operator owns its uptime.
2. URL unset → probe `[server].addr`. If a daemon is already
   listening, use it.
3. Probe fails → fork-exec `kenn server start --idle-timeout
   600` and re-probe.
4. Spawn fails or never becomes healthy in 5 s → in-process
   `LlamaEmbedder` fallback for this process. (The spawned child
   may still come up and serve other processes.)

Two concurrent spawns race to `bind`. The loser exits cleanly on
`EADDRINUSE`; the loser-client's next probe finds the winner.

## Endpoints (v1)

| Method | Path | Notes |
|---|---|---|
| `GET` | `/healthz` | Returns 200 once the host is wired. Does NOT reset the idle counter (so polling clients don't keep the daemon alive forever). |
| `POST` | `/v1/embeddings` | OpenAI-compatible. **Defaults to `encoding_format: "base64"`** (kenn deviation from OpenAI's `"float"` default — bit-exact f32, ~3× smaller wire). Clients wanting float arrays must request them explicitly. See [embeddings.md](embeddings.md). |
| `GET` | `/v1/models` | OpenAI-compatible. v1 advertises exactly one model — the configured `[embeddings].model`. Reports the id without loading the model. |

## Logging

`tracing` output goes to stderr in foreground mode, and to
`<state_dir>/server.log` in daemon mode (10 MB × 3 rotating
files). Filter via `RUST_LOG=kenn_server=debug` (standard
`tracing_subscriber::EnvFilter` syntax).

## Future capabilities

Mentioned here only to clarify the host's shape — none are
implemented in v1:

- **Inter-agent communication.** A route family for
  message-passing between named agent processes running against
  the same user account.
- **Shared user history / memory from hooks.** An event ingest
  API + per-user aggregation store fed by Claude Code hooks
  (and similar).

Both will register as sibling modules on the same host — same
port, same PID, same `/healthz`.
