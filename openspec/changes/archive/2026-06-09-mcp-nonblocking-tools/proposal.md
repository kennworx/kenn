## Why

MCP tool handlers must be fast and must never hang — agents abandon the
tool the moment a call stalls. A code audit (plus thread-sampling a live
server) found handlers that can exceed a reasonable latency budget or
block other calls indefinitely under concurrency:

- `with_findings` takes a `tokio::Mutex` and holds it **across** its async
  closure. But the findings read methods (`search_findings`, `get_finding`,
  the DAG walks) take `&self` — so reads don't need exclusive access at
  all. The `Mutex` **over-locks reads**, serializing every findings tool
  behind whatever call holds the lock. This is the concurrency stall we
  observed (a second findings call blocked behind a first).
- `search_findings` loads **all** findings into memory and scores every
  one with no bound — hundreds of ms on a large store, held under that
  lock.
- There is **no per-request logging**, so diagnosing a stall required
  attaching a sampler to the process — the failure was invisible.

The invariant: **every MCP tool returns well under ~200ms except
`wait_for_index` (own timeout), and no tool blocks another indefinitely.**

(A fourth audit finding — synchronous SQLite running on runtime worker
threads — is split into its own change, `mcp-offload-blocking-storage`,
because it reopens a documented "async-native, no `spawn_blocking`"
decision and was *not* the cause of any observed hang.)

## What Changes

- **Findings store becomes an `RwLock`** (from `Mutex`): read tools take a
  shared `.read()` and run concurrently; only mutating tools take
  `.write()`. This is the primary fix — the read path is already `&self`,
  so reads should never have serialized.
- **Persistent findings search index** — `search_findings` queries a
  persistent FTS5 index (`<derived_root>/findings.db`, built at open /
  maintained on write), bounded by `LIMIT`, resolving only the top-`limit`
  records. The read path never builds a transient index/table (that was the
  per-call `CREATE TABLE` + full-corpus load it replaces). *(Reshaped from
  "bound search" per the steering: reads use a persisted index, never build
  one in the read path.)*
- **Per-tool observability via the stack** — one `tracing` span per tool
  call (name + duration) at the dispatch boundary, plus a `metrics` facade
  (counter + duration histogram), emitted to stderr/exporter never stdout.
  *(Reshaped from "log a line" per the steering: unify on spans + metrics.)*

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `mcp-server`: add a non-blocking-dispatch invariant (no lock held across
  slow/unbounded work; concurrent tools don't serialize) and per-request
  duration logging.
- `findings-store`: `search_findings` is bounded (no full-corpus load), and
  concurrent reads do not block each other.

## Impact

- **Code:** `crates/kenn-mcp/src/tools/state.rs` (`findings` field →
  `RwLock`; split `with_findings` into read/write variants and route each
  findings tool), `crates/kenn-mcp/src/server.rs` (per-call duration
  logging), `crates/kenn-store/src/db/findings/store.rs` (`search_findings`
  bound). No wire/schema changes.
- **Behavior:** concurrent findings tools no longer serialize; tool latency
  is logged. All tool input/output shapes and error codes are unchanged.
- **Not in scope:** offloading synchronous SQLite off the runtime
  (`mcp-offload-blocking-storage`); the client/transport stall observed in
  one session (the kenn server was provably idle); `get_source` bodies or
  new search tools.
