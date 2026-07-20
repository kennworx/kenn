## Why

The storage layer (`kenn_store::db::ReadDb` and `SurrealdbSink`) exposes
a synchronous API but internally manages its own
`tokio::runtime::Runtime` and uses `block_on` for every query. That was
fine when the only caller was the `kenn index` CLI. With `kenn mcp`
landing as an async server (`mcp-owned-indexing`), every storage call
now nests a tokio runtime inside rmcp's runtime — which panics
("Cannot start a runtime from within a runtime") unless the caller goes
through `tokio::task::spawn_blocking`.

The MCP foundation layered around this with a blocking shim:
- Every tool dispatch goes through `spawn_blocking` so the inner
  `block_on` doesn't nest.
- `ServerState::bootstrap_blocking` exists solely to open a `ReadDb`
  outside an async runtime context.
- An entire chapter of comments explains the nesting hazard.

This is a smell. With async-native storage:
- MCP tools `await` naturally — no `spawn_blocking` per call.
- Tokio's blocking-thread pool is no longer consumed per query.
- `bootstrap_blocking` and the nesting-hazard comments disappear.
- The CLI wraps once at the top level (`rt.block_on(serve_cli())`)
  instead of every storage call paying the bridge cost.

## What Changes

- **BREAKING (internal)**: `kenn_store::db::ReadDb` becomes async.
  Every method (`fetch_symbol`, `count_table`, `distinct_languages`,
  etc.) is now `async fn`. The struct no longer owns a `Runtime`.
- **BREAKING (internal)**: `kenn_store::db::SurrealdbSink` becomes
  async at construction (`async fn create`, `async fn create_with_options`).
  The `Sink` trait it implements stays synchronous (the pipeline is
  sync; we run it under `block_on` from the async caller). Internally
  every query uses the caller's runtime via `await` instead of
  `self.rt.block_on(...)`.
- `kenn-mcp` removes `ServerState::bootstrap_blocking` and stops
  routing tools through `spawn_blocking`. `with_db` becomes async.
- `kenn-cli`'s `cmd_index` and `cmd_mcp` wrap their async bodies in
  `rt.block_on(...)` once at the top.
- `tools.rs::ServerState` keeps its `RwLock<LifecycleState>` but the
  lock guard is held briefly across awaits (using `tokio::sync::RwLock`
  if needed for cooperative yielding).
- `kenn_store::workflow::index_workspace` becomes `async fn`. The
  pipeline call within stays sync — the workflow wraps it in
  `tokio::task::spawn_blocking` (the pipeline is CPU-bound and needs a
  blocking thread anyway). The workflow's other steps (config read,
  store open, lifecycle calls) are sync I/O and stay sync.

## Capabilities

### New Capabilities

None. This is a refactor of existing capabilities' implementation
shape — the wire-level contracts (MCP tool inputs/outputs, CLI flags,
on-disk format) are unchanged.

### Modified Capabilities

- `mcp-server`: tool dispatch path changes. Tools no longer route
  through `spawn_blocking`; `with_db` becomes async. The wire-level
  tool contract is unchanged.
- `mcp-orchestrated-indexing`: `bootstrap_blocking` goes away. The
  startup orchestration awaits `index_workspace` directly. The wire-
  level lifecycle behavior (startup states, INDEX_UNAVAILABLE,
  progress notifications) is unchanged.

## Impact

- **Code**:
  - `crates/kenn-store/src/db.rs` — drop the inner `Runtime`. Every
    `pub fn` that wraps `block_on` becomes `pub async fn`. The
    `Sink` trait impl stays sync (its callers are the sync pipeline);
    its body uses `tokio::runtime::Handle::current().block_on` (or
    equivalent) when called from a sync caller that has a runtime.
  - `crates/kenn-store/src/workflow.rs` — `index_workspace` becomes
    async; wraps the synchronous `run_pipeline_with_progress` in
    `spawn_blocking`.
  - `crates/kenn-mcp/src/tools.rs` — `with_db` async; tools become
    async; `bootstrap_blocking` removed.
  - `crates/kenn-mcp/src/server.rs` — `run_tool` no longer uses
    `spawn_blocking` for the tool body; it calls the async tool fn
    directly. Notification pump unchanged.
  - `crates/kenn-mcp/src/indexing.rs` — startup orchestration
    becomes a single `tokio::spawn`'d async task rather than
    `spawn_blocking`. Within, the pipeline call is wrapped in its
    own `spawn_blocking`.
  - `crates/kenn-cli/src/cmd_index.rs` — wraps the body in
    `rt.block_on` once; calls become `.await`.
  - `crates/kenn-cli/src/cmd_mcp.rs` — already runs inside a
    multi-thread runtime; tool of awaits now just propagate.
- **APIs (storage)**: every public method on `ReadDb` and
  `SurrealdbSink` is now async. Callers within the same workspace are
  the only consumers; no external API change.
- **APIs (MCP/CLI)**: no external surface change. CLI args, JSON-RPC
  tool contracts, and on-disk layout all unchanged.
- **Schema / on-disk**: no change.
- **Performance**: small expected wins on MCP read path (no
  spawn_blocking thread allocation per tool call) and on the
  shutdown path (no per-sink runtime to spin up and tear down).
  No regression expected on the bulk-write pipeline (it's still
  sync end-to-end, with one block_on wrapper at the workflow
  boundary).
- **Risk**: every callsite touched. The refactor is large but
  mechanical (sync→async rewrite). Test coverage from the
  `mcp-owned-indexing` foundation catches contract regressions.
