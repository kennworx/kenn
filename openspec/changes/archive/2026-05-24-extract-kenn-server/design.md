# Design — extract-kenn-server

## Decisions

### D0: Thin host + capability modules

The kenn server is a tiny HTTP host (axum) plus N capability
modules. The host owns the listener, addr, PID file, logging,
`/healthz`, idle-timeout aggregator, and graceful shutdown. Each
module owns its routes, state, and its own config section. v1
registers one module: embeddings.

Future capabilities (agent-comms, hook-fed memory) need the same
lifecycle and addr; separate daemons would multiply ports, PID
files, and auto-spawn logic. One host, many modules keeps the
operational surface flat. Cost: modules share the host's
idle-timeout (aggregate across all routes) and single port — fine
for v1; the host can grow per-module opt-outs later.

### D1: OpenAI `/v1/embeddings` wire format with base64 by default

JSON over HTTP/1.1, mirroring OpenAI's embeddings shape. Same
client code works against kenn, ollama, lm-studio, and hosted
endpoints.

The kenn **server defaults to `encoding_format: "base64"`** when
the request omits the field — a deliberate deviation from
OpenAI's `"float"` default. The reasoning is two-fold:

1. **Bit-exact f32 round-trip.** base64 carries the raw 4-byte
   little-endian representation of each f32, so the client
   reconstructs exactly what the model produced. The float path
   serializes f32 → JSON number (ryu shortest) → re-parses as
   f64 in most JSON libraries → casts back to f32. The ryu-shortest
   round-trips through f32 exactly, but a careless client that
   keeps f64 throughout (or compares against base64-derived f32)
   will see ~1e-9 drift. base64 eliminates the foot-gun.
2. **~3× smaller wire payload.** A 768-dim vector is ~3 KB as
   JSON floats vs ~1 KB as a base64 string; parsing is faster too.

The kenn **client (`RemoteEmbedder`) always sends
`encoding_format: "base64"` explicitly** in the request body.
Against kenn's own server the field is redundant with the
default, but it makes the same client work uniformly against
ollama / lm-studio / OpenAI (all of which default to `"float"`
when the field is omitted).

Clients that want JSON-float arrays (e.g. third-party tools
pointed at a kenn server) send `encoding_format: "float"`
explicitly.

gRPC remains a possible follow-up if HTTP framing ever becomes
the bottleneck.

### D2: Two-branch selector — URL set vs unset

```
embeddings.url resolved (env > config)?
  yes → RemoteEmbedder(url), no spawn, degrade on failure
  no  → probe [server].addr
        up   → RemoteEmbedder(local)
        down → fork `kenn server start`, retry probe
               spawn fails → LlamaEmbedder (in-process)
```

URL set means the user has an external provider; kenn must not
also fork its own server. URL unset means kenn manages its own
embedding lifecycle. The in-process fallback preserves
offline-degrades-cleanly when forking can't work (no PATH, no
exec permission, etc.).

### D3: Fixed default port (`41873`)

Client and server agree on one address without coordination, no
PID-file-parse on the probe path. Conflicts surface as a clean
`EADDRINUSE` on bind → spawn-fail → in-process fallback. `41873`
is non-round, unregistered, no known conflict. Discovery-file
and UDS were considered; deferred until a real conflict.

### D4: Spawn race resolved by `bind`

Concurrent spawns both probe-fail, both fork; both call `bind`;
OS arbitrates; the loser exits on `EADDRINUSE`; the loser-client
re-probes and connects to the winner. No lockfile — the bind is
the atomic step. The wasted fork+exec on a race is rare;
steady-state has one server running and probes succeed.

### D5: Manifest carries only the model id

No provider URL (where embeddings come from is a runtime
concern, not a property of the vectors). No content hash
(`gguf_xxh3` only worked for the in-process path; remote
providers expose only a name). Asymmetric identity ("hash if you
can") would mean two manifests with the same id but different
hash-presence clash awkwardly. Picking the weaker, universal
scheme is cleaner. Cost: model swap under an unchanged id is
undetected — mitigated by the universal id-versioning convention
(`-v1` → `-v2`).

### D6: Global config, separate file, separate struct

Per-workspace `kenn.toml` is checked into git; user-wide
embedding/server settings have no business in a shared repo.
Schemas don't overlap and never should. `kenn-config` already
owns TOML parsing; adding a second loader keeps it reviewable in
one place. Precedence on every field: env > global config >
default.

### D7: State paths via `directories`

PID file, logs, and future per-user state files live in the
per-OS state dir (XDG `state_dir` / macOS `Application Support`
/ Windows `LocalAppData`). Distinct from the config dir (D6) by
OS convention — config is user-edited, state is machine-managed.
`directories` is the most-used crate of the three competing
options; pinning here avoids a future bikeshed.

### D8: Daemon idle lifecycle is mode-dependent

Presence of `--idle-timeout N` on the `kenn server start`
command line enables process-idle exit and sets the duration;
absence disables it. The auto-spawn helper always passes the
flag (default 600 s); humans and supervisors normally don't. The
existing in-daemon `LazyEmbedder` (60 s model-idle release)
nests inside this: model unloads after 60 s of no embed calls,
process exits after 10 min of total idleness.

### D9: PID file authoritative over HTTP health

`stop` reads `<state_dir>/server.pid`, sends SIGTERM, polls with
a grace, then SIGKILL. HTTP-only shutdown means a hung server
can't be killed — the PID file is the OS-level escape hatch.
`/healthz` exists for spawn-readiness probing, not shutdown.

### D10: Single inference worker with micro-batch coalescing

The daemon runs one inference worker behind a bounded channel.
`llama-cpp-2` on Metal is effectively single-context, so
parallel inference within one process buys nothing.

The worker MAY coalesce concurrent `/v1/embeddings` requests
into one llama.cpp batch when multiple requests are queued: pull
everything in the queue (up to a small bound), submit as one
batched inference, fan results back to each caller's oneshot.
Under MCP fan-out (N attachments issuing single-string queries
concurrently), the latency for each request approaches one
inference call rather than N. Per-request batches stay
per-request; a reindex's large array doesn't get diluted into
unrelated queries.

Concurrent reindex + MCP query → FIFO wait at the boundary
between batches; user gets a clearly slower query rather than
mysteriously interleaved results. Priority queue (queries
preempt batch work) is a follow-up if the FIFO wait bites.

### D11: `GET /v1/models` reports the configured id without loading the model

Returns the resolved `[embeddings].model` id from config and
exits — no model load. The auto-spawn helper needs to confirm
the server is up *before* paying model-load latency, and
`/v1/models` is the natural "is the embedding provider serving
the model I expect" check. Trade-off: a misconfigured model file
is detected on the first `/v1/embeddings` call (5xx with the
load error), not at `/v1/models` time. Same property holds for
ollama and lm-studio.

### D12: Daemon logs to `<state_dir>/server.log` with size-bounded rotation

A detached daemon has no stdout/stderr to `tail`. The daemon
writes `tracing` output to a rotating file (10 MB × 3). Cross-OS
uniformity beats a per-platform journal integration; a journald
sink can land later behind a feature flag.

### D13: Remote-provider failures always degrade, never bubble

`LlamaEmbedder` maps **load** failure to `Ok(None)` (search
degrades to lexical-only) but lets **inference** failure bubble
as `Err(DbError)` — the asymmetry made sense when the embedder
was always local. `RemoteEmbedder` maps **all** failure classes
to `Ok(None)`: unreachable, non-2xx, malformed body, timeout. A
remote provider is a moving target outside the kenn process's
control; the user wants search to keep working in degraded mode,
not to fail their reindex. Cost: reduced observability — the
producer SHALL log the cause at WARN. A future
`KENN_EMBED_STRICT=1` mode could bubble errors for users who
prefer hard failures.

## Risks

### R1: Auto-spawn races with daemonization

Auto-spawn forks `kenn server start` (daemon mode). On Unix the
parent exits after fork; on Windows `CREATE_NEW_PROCESS_GROUP |
DETACHED_PROCESS`. The spawn helper must poll `/healthz` for
readiness, not the spawned PID — and must time out and fall back
to in-process if the daemon never comes up.

### R2: Model-id mismatch between client and shared daemon

Two clients of the same user with different `KENN_EMBED_MODEL`
pointing at the same daemon: daemon advertises one id; the
mismatched request returns 404. Per the once-per-process
selection rule, the producer can't swap mid-flight — the 404 is
treated as any other remote failure (`Ok(None)`, lexical-only,
no manifest stamp). Detectable in logs rather than silent
wrong-model corruption. Dissolved by future multi-model daemon
support or runtime producer-swap.

### R3: `id`-only identity hides quantization changes

Bundled model going from `q8_0` to `q4_K_M` GGUF under the
unchanged id silently invalidates vector compatibility.
Maintainer-side mitigation: bump the id whenever the bundled
weights change (`-q8` → `-q4`). Document in the model-update
process.

### R4: Shared multi-user hosts silently route across users

`directories::state_dir()` is per-user, but `127.0.0.1:41873` is
not, and v1 has no auth:

1. User A's MCP auto-spawns a daemon; daemon binds the port.
2. User B's MCP probes the port — **User A's daemon responds**.
3. User B's selector chooses `RemoteEmbedder` against the probed
   address. No spawn, no bind, no `EADDRINUSE`.
4. User B's embed traffic (source code, doc text, finding
   contents) flows through User A's daemon process.

A data-isolation hole on multi-user boxes, not just a sharing
degradation. Mitigation in v1: shared-host users **MUST** set
`KENN_SERVER_ADDR=127.0.0.1:<unique-port>` per user (and matching
`[server].addr`). The documentation task surfaces this
prominently. A real fix (per-user UDS, uid-derived port,
`/healthz` uid check) waits until shared-host use is observed
and one can be picked deliberately.

## Open questions

(none open at the v1 scope.)
