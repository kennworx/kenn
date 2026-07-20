## Context

The `mcp-owned-indexing` change landed an async MCP server on top of a
synchronous storage layer. To prevent the `block_on` inside
`kenn_store::db` from nesting a runtime inside rmcp's runtime, we
shimmed every storage call with `tokio::task::spawn_blocking` and added
`ServerState::bootstrap_blocking` to open the DB outside the rmcp
runtime context. The shim works but is awkward:

- Every tool call costs a tokio blocking-pool slot.
- `bootstrap_blocking` cannot be called from inside an async runtime —
  the wrong call site panics with "Cannot start a runtime from within
  a runtime."
- The storage layer's per-instance `Runtime` is created and torn down
  for every `ReadDb`/`SurrealdbSink`, even though the caller already
  has a runtime.

This change makes the storage layer async. MCP awaits naturally; the
CLI wraps once at the top level.

## Goals / Non-Goals

**Goals**

- `kenn_store::db::ReadDb` exposes `async fn` methods. No internal
  `Runtime` ownership.
- `kenn_store::db::SurrealdbSink::create` and `create_with_options`
  become async. Internally the per-batch `flush_batch` continues to
  use the caller's runtime via `block_on` (it's invoked from the sync
  `Sink` trait inside `run_pipeline`, which itself is launched on a
  blocking thread by the async caller).
- MCP tool dispatch becomes async end-to-end. `spawn_blocking` and
  `bootstrap_blocking` go away.
- CLI commands (`cmd_index`, `cmd_mcp`) wrap their bodies in a
  single top-level `rt.block_on(...)` call.

**Non-Goals**

- Changing the `Sink` trait shape. It stays sync — the pipeline is a
  long-running synchronous CPU-bound loop, and converting every
  pipeline-internal call to async would balloon the diff with no
  perf benefit.
- Changing the wire-level MCP tool contract or the JSON-RPC error
  shapes.
- Changing on-disk format, snapshot lifecycle, or kenn-dotnet wire
  protocol.
- Dropping the SurrealDB embedded engine or RocksDB backend.

## Decisions

### D1. `ReadDb` becomes async

Every `pub fn` on `ReadDb` that wraps `self.rt.block_on(async { ... })`
becomes `pub async fn` and the body runs on the caller's runtime.

```rust
// before
impl ReadDb {
    pub fn fetch_symbol(&self, lang: Language, key: &str) -> Result<Option<SymbolRow>, Error> {
        self.rt.block_on(async { ... })
    }
}

// after
impl ReadDb {
    pub async fn fetch_symbol(&self, lang: Language, key: &str) -> Result<Option<SymbolRow>, Error> {
        ...await...
    }
}
```

The `rt` field is removed. `ReadDb::open` becomes async (it issues a
SurrealDB connection setup query). `ReadDb` stores only the
`Surreal<Db>` handle.

Alternative considered: keep `ReadDb` sync, move the `Runtime` to a
shared per-process singleton. Rejected — singletons have lifecycle
hazards (test isolation, drop ordering) and the per-call `block_on`
cost remains.

### D2. `SurrealdbSink` becomes async at construction

`SurrealdbSink::create` and `create_with_options` become async. The
sink stores a `Surreal<Db>` handle plus its `SinkOptions`; no
`Runtime` field.

The `Sink` trait impl (`begin_run`, `write_batch`, `end_run`) stays
sync. Its body needs to run async SurrealDB queries; we use
`tokio::runtime::Handle::current().block_on(...)` from inside the
sync method. This works because:

1. The pipeline is launched from `tokio::task::spawn_blocking`, which
   runs on a tokio blocking thread. The thread has access to the
   parent runtime via `Handle::current()`.
2. `block_on` from a blocking thread is allowed (unlike from a tokio
   worker thread, which is the case we hit before).

This is the asymmetry: caller of the sink is sync (the pipeline);
caller of the workflow that drives the pipeline is async (MCP /
spawn_blocking from CLI). The handle bridge is the bridge.

Alternative considered: make the entire pipeline + Sink trait async.
Rejected — the pipeline is ~600 LOC of sync logic, and async
propagates virally. The blocking-thread + `Handle::current().block_on`
pattern is the established Rust idiom for this exact bridge.

### D3. MCP tool dispatch becomes async

`ServerState::with_db` becomes:

```rust
pub(crate) async fn with_db<R, F, Fut>(
    &self,
    f: F,
) -> Result<R, McpError>
where
    F: for<'a> FnOnce(&'a ReadyView<'a>) -> Fut,
    Fut: std::future::Future<Output = Result<R, McpError>>,
```

Tools become `pub async fn`. Tool wrappers in `server.rs` `#[tool]`
methods drop `run_tool`'s `spawn_blocking` and just `.await` the tool
function.

The lifecycle lock stays `std::sync::RwLock` — the critical sections
are short and don't suspend. Held only across the body of `with_db`'s
match (which now extends across `f.await`). For correctness, we need
to be careful: `RwLockReadGuard` is `!Send` if any inner field is
`!Send`, which would block awaiting across the boundary. We refactor
to clone the needed values (snapshot_id, indexed_at) out of the
guard and re-borrow `read` only for the closure body.

Alternative considered: switch lifecycle lock to `tokio::sync::RwLock`.
Rejected — that lock is async and would require `await` to acquire.
For our pattern (write rarely from the indexing task; read on every
tool call), `std::sync::RwLock` with copy-out semantics is simpler.

### D4. `bootstrap_blocking` is removed

Replaced by an async fn `ServerState::bootstrap()` that callers can
`.await` from any tokio context. The synchronous startup decision in
`indexing.rs` becomes async; its `spawn_blocking` for the pipeline call
remains (the pipeline is CPU-bound).

In-process tests that previously called `bootstrap_blocking` now use
`#[tokio::test]` and `state.bootstrap().await`.

### D5. CLI wraps once at the top

```rust
// before
pub fn run(workspace: &Path) -> Result<ExitCodes> {
    // sync body, calls into sync storage
}

// after
pub fn run(workspace: &Path) -> Result<ExitCodes> {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(run_async(workspace))
}

async fn run_async(workspace: &Path) -> Result<ExitCodes> {
    // body is now async; storage calls await naturally
}
```

`cmd_mcp` already builds a runtime; its body just becomes `.await`-
based. `cmd_index` adopts the same shape.

## Risks / Trade-offs

- **Risk: every storage call site touched.** Mechanical conversion
  but tedious. → Mitigation: existing test coverage (workspace tests,
  end-to-end, lifecycle) catches regressions; we run the full
  workspace suite at each milestone.
- **Risk: `Sink` trait blocking on `Handle::current()`.** If callers
  ever invoke `flush_batch` from a context without a tokio runtime
  (e.g. a non-async test), `Handle::current()` panics. →
  Mitigation: document the requirement; tests that drive the sink
  set up a `#[tokio::test]` runtime.
- **Risk: lifecycle `RwLockReadGuard` Send-ness across awaits.**
  Holding a `std::sync::RwLockReadGuard` across `.await` is allowed
  by the compiler but blocks other readers/writers cooperatively.
  → Mitigation: pattern-match and copy the small fields out of the
  guard before awaiting. Hold the guard only for the match itself.
- **Trade-off: minor diff churn for the SurrealDB sink path.** The
  `Sink` trait stays sync, so the bulk-write performance characteristic
  is unchanged. The async wrapper is at the workflow boundary.

## Migration Plan

No data migration. No on-disk format changes.

Rollout sequence (single change, but tractable in stages):
1. `kenn_store::db::SurrealdbSink::create` async + `Sink` impl uses
   `Handle::current().block_on`. Existing callers updated.
2. `kenn_store::db::ReadDb` methods async. CLI's `cmd_status` and
   `kenn-mcp::tools` updated.
3. `kenn_store::workflow::index_workspace` async. Callers updated.
4. `kenn_mcp` tools/server/indexing converted; `bootstrap_blocking` /
   `spawn_blocking` for tools removed.
5. `kenn-cli::cmd_index` / `cmd_mcp` wrap `rt.block_on(...)`.

Rollback: git revert.

## Open Questions

None blocking. Possible future work:
- Convert the indexing `Sink` trait to async (would simplify the
  workflow boundary but ripples into 600 LOC of pipeline code).
- Drop the per-sink `Runtime` ownership entirely once the pipeline
  side moves async too.
