## Why

The MCP storage path is framed as "async-native," and the `mcp-server`
spec says tools "MUST NOT" use `spawn_blocking`. But the path is actually
**synchronous under the hood**: `ready_view_or_err` calls
`SqliteReader::connect()` (a blocking `Connection::open_with_flags`) on
every tool call, and the `DbConn` query methods (`count_table`, the
search/scan reads) are `async fn`s that execute synchronous rusqlite while
holding a `Mutex<Connection>`. They therefore **block a runtime worker
thread** for the duration of the I/O, and a single `DbConn` serializes
concurrent reads behind its mutex. Under slow or contended disk a tool call
can exceed the latency budget and tie up a worker, breaking the "every tool
< ~200ms, only `wait_for_index` blocks" invariant.

This was a code-audit finding, not the cause of any observed hang (the
sampled server was idle, not blocked in SQLite) — so it is split from the
`mcp-nonblocking-tools` change and addressed deliberately here, because it
**reverses a documented decision**.

A pilot (`crates/kenn-store/examples/async_sqlite_spike.rs`, finding
`fnd_bb3ef79e`) validated **async-sqlite's `Pool`** as the replacement: a
round-robin set of background-thread rusqlite `Client`s. `pool.conn(f).await`
ships the closure to a worker thread over a channel — the blocking SQLite
call never runs on a runtime worker, and N connections serve N concurrent
reads in parallel (measured ~3–4× at `num_conns=4`). The pilot also
confirmed vec0 works through pool-opened connections and that `findings.db`
can be ATTACHed read-only to each pooled connection.

## What Changes

- **Revise** the `mcp-server` "Tool dispatch is async end-to-end"
  requirement: the `spawn_blocking` ban was meant to prevent nested-runtime
  hacks, not to forbid moving genuinely-blocking SQLite work off the runtime
  workers. The storage read path SHALL run on a dedicated connection pool so
  blocking SQLite never occupies a runtime worker, while still not wrapping
  whole async fns in `spawn_blocking` to dodge a nested runtime.
- **Hold an async-sqlite `Pool` on the `ReaderBinding`**, opened once when a
  snapshot is bound (`open_ready_if_live` and the cross-instance reload
  path), read-only over `code.db`/`vector.db`, with vec0 registered and
  `findings.db` ATTACHed read-only to each connection. Tool reads run via
  `pool.conn(...).await` instead of opening a fresh `DbConn` per call.
- **Retire the per-call `connect()`** on the hot path: `ready_view_or_err`
  hands out a cheap handle to the binding's pool rather than doing a
  blocking `open_with_flags`. The pool is torn down when the binding is
  swapped out on snapshot rotation.
- **Wire contract unchanged** — same payloads, same `INDEX_UNAVAILABLE` /
  `EMPTY_SNAPSHOT` and error forms, same pagination/progress.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `mcp-server`: the async-dispatch requirement is revised — the read path
  runs on a per-snapshot connection pool so blocking SQLite stays off the
  runtime workers and concurrent reads parallelize.

## Impact

- **Code:** `crates/kenn-mcp/src/tools/state.rs` (`ReaderBinding`,
  `ready_view_or_err`, `with_db`, the `ReadyView` handle),
  `crates/kenn-mcp/src/state.rs` (`ReaderBinding`),
  `crates/kenn-store` SQLite reader (a pool-backed reader alongside / in
  place of the single-`DbConn` `connect()`). New dependency: `async-sqlite`
  (0.6, `default-features=false, features=["bundled"]` — unifies with the
  workspace `rusqlite 0.40`).
- **Behavior:** blocking SQLite open/query no longer stalls a runtime
  worker; concurrent tool reads no longer serialize behind one connection;
  tool latency under I/O pressure improves. Tool outputs identical.
- **Dependency:** independent of `mcp-nonblocking-tools`, but both serve the
  same latency invariant. Builds on the validated pilot.
