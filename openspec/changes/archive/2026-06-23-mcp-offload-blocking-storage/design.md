## Context

The MCP "async-native" storage path is synchronous under the hood.
`ready_view_or_err` (`crates/kenn-mcp/src/tools/state.rs`) calls
`SqliteReader::connect()` (`crates/kenn-store/src/db/sqlite/reader/projection.rs`,
a sync `Connection::open_with_flags`) on **every** tool call, then `with_db`
runs `count_table("symbols").await` and the tool's reads against the
resulting `DbConn`. `DbConn` wraps a single `Mutex<Connection>`, so its
`async fn` methods (a) execute synchronous rusqlite on the runtime worker
and (b) serialize concurrent reads behind the mutex. The existing
`mcp-server` requirement bans `spawn_blocking`, so the status quo blocks the
runtime by design.

The `async_sqlite_spike` pilot (finding `fnd_bb3ef79e`) validated
async-sqlite's `Pool` as the fix: background-thread `Client`s, round-robin
`pool.conn(f).await`, vec0 through pooled connections, read-only WAL ATTACH
of `findings.db` with a live writer, and ~3–4× parallel speedup at
`num_conns=4`.

## Goals / Non-Goals

**Goals:**
- Blocking SQLite open/query on the tool path runs off the runtime worker
  threads, so it can't stall a worker or exceed the latency budget.
- Concurrent tool reads parallelize instead of serializing behind one
  `DbConn` mutex.
- The async-dispatch requirement is revised to allow this, with the original
  nested-runtime guard preserved.

**Non-Goals:**
- Rewriting every reader query to be natively async SQL. We keep synchronous
  rusqlite inside the `pool.conn` closures; only *where* they run changes.
- The findings RwLock / bounded-search / logging work (that is
  `mcp-nonblocking-tools`, already landed).
- Moving the bulk-scan graph projection (the in-RAM CSR built at bind time)
  onto the pool — that is a one-time load, not a per-call hot path.

## Decisions

### D1: An async-sqlite `Pool` lives on the `ReaderBinding`

The `ReaderBinding` (per-snapshot, GC-pinned, swapped on hot-reload) gains a
`Pool` opened once when the snapshot is bound — in `open_ready_if_live` and
the cross-instance reload path. The pool is read-only (`SQLITE_OPEN_READ_ONLY
| SQLITE_OPEN_URI`) over the snapshot's `code.db` / `vector.db`, with vec0
registered process-globally (existing `ensure_vec_extension`) and
`findings.db` ATTACHed read-only to **each** connection via `conn_for_each`
at pool build. `num_conns` is small and fixed (pilot used 4; final value
tuned to the read concurrency we expect).

`ready_view_or_err` stops doing a blocking `connect()` per call: it hands the
`ReadyView` a cheap clone of the binding's `Pool` (Pool is `Arc`-backed,
`Clone`). Tool reads become `view.pool.conn(|c| { ... }).await`. When the
binding is swapped on rotation, dropping it drops the pool (closing the
background connections).

*Alternative considered:* per-call `spawn_blocking` around the existing
`connect()` + `DbConn` calls (the original framing of this change). Rejected
— it offloads the blocking syscall but still opens a connection per call and
still serializes nothing/everything ad hoc; the pool gives connection reuse
**and** real read parallelism in one structure, validated by the pilot.

*Alternative considered:* tokio-rusqlite / sqlx. Rejected earlier —
tokio-rusqlite is version-blocked at rusqlite ^0.37; sqlx is a full rewrite
plus vec0 re-integration risk. async-sqlite pins rusqlite 0.40 (unifies with
the workspace) and is MIT.

### D2: the reader pool is code-only; `findings.db` is NOT attached

*(Revised during implementation.)* The pilot validated read-only WAL ATTACH
of `findings.db`, but the implementation found it unnecessary: the findings
tools go through the `FindingsStore` (its own store) plus a code-graph
resolver built from a `code.db` query (`code_node_resolver`), so
`search_findings` staleness never needs `findings.db` and `code.db` in one
SQL statement. The reader pool therefore attaches only `vector.db`, keeping
it code-only and avoiding coupling the findings writer's lifetime into the
per-snapshot pool. If a future feature needs a cross-store join, the pilot
shows the read-only ATTACH path works with the live findings writer (and
that `immutable=1` is unsafe while WAL frames are pending).

### D3: Preserve the wire contract exactly

The change is about *where* and *on how many connections* the blocking call
runs. Payloads, error codes (`INDEX_UNAVAILABLE`, `EMPTY_SNAPSHOT`),
pagination, and progress notifications are untouched. A regression test pins
`get_workspace_overview` output and the `Indexing`-state error form.

## Risks / Trade-offs

- [Pool open cost at bind time] → paid once per snapshot bind (off the hot
  path), not per tool call; replaces N per-call `connect()`s with N pooled
  connections opened once.
- [`num_conns` sizing] → too few serializes reads; too many wastes fds/memory
  on idle background threads. Start small (≈4), revisit if reads queue.
- [Read-only WAL ATTACH depends on a live findings writer] → that is the
  production invariant (the writer is held for the server lifetime); the
  writer-gone edge is documented in the pilot but not a supported path.
- [Reversing the documented "no spawn_blocking" rule] → the revised
  requirement keeps the nested-runtime guard; we move blocking work onto a
  dedicated pool (not `spawn_blocking`), and the rationale is in the delta.

## Open Questions

- Final `num_conns` — fixed small constant vs. derived from
  `available_parallelism`. Default to a small constant; measure under
  concurrent tool load before tuning.
- Whether the bulk-scan graph projection should also read through the pool at
  bind time or keep its own one-shot connection (leaning: keep its own — it's
  a single load, not a hot path).
