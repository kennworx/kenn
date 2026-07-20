## 1. SurrealdbSink: async construction, sync `Sink` impl

- [x] 1.1 Drop the `rt: Runtime` field from `SurrealdbSink`. The
      sink stores `path`, `db: Option<Surreal<Db>>`, `bench`, and
      `options`.
- [x] 1.2 Convert `SurrealdbSink::create` and `create_with_options`
      to `async fn`. Internally `await` `Surreal::new::<RocksDb>` /
      `Surreal::new::<Mem>` instead of wrapping in `block_on`.
- [x] 1.3 Convert `SurrealdbSink::open` to `async fn`. Apply schema
      via `await`.
- [x] 1.4 Inside the `Sink` trait impl (`begin_run` / `write_batch`
      / `end_run`), replace `self.rt.block_on(async { ... })` with
      `tokio::runtime::Handle::current().block_on(async { ... })`.
      Document at the trait boundary that the caller MUST be running
      on a tokio runtime (typically a blocking thread).
- [x] 1.5 Convert `flush_batch` and the per-table bench-mode block
      to use `Handle::current().block_on`.
- [x] 1.6 Convert `count_table`, `fetch_symbol_pub_id`, and any
      other test-only sync methods to async.
- [x] 1.7 Convert `shutdown` to async; it now `.await`s the no-op
      query that flushes RocksDB's WAL. Drop the `Drop` impl that
      replaces the runtime — without an inner runtime there's no
      special teardown to perform.
- [x] 1.8 Update existing kenn-store tests (`round_trip_via_sink_trait`,
      `defer_fulltext_indexes_buildable_in_same_lifecycle`, etc.) to
      `#[tokio::test]` and `.await`.

## 2. ReadDb: fully async API

- [x] 2.1 Drop the `rt` field from `ReadDb`. The struct stores only
      the `Surreal<Db>` handle.
- [x] 2.2 Convert `ReadDb::open` to `async fn`. The connection
      handshake (`use_ns`, `use_db`) awaits.
- [x] 2.3 Convert every `ReadDb` `pub fn` that wraps `block_on` to
      `pub async fn`. Bodies replace `block_on(async { ... })` with
      direct `.await`.
- [x] 2.4 Document at the type level that `ReadDb` requires an
      ambient tokio runtime (multi-thread is recommended for
      concurrent tool dispatch).

## 3. Workflow: async wrapper around sync pipeline

- [x] 3.1 Convert `kenn_store::workflow::index_workspace` to
      `async fn`.
- [x] 3.2 Wrap the sync `run_pipeline_with_progress` invocation in
      `tokio::task::spawn_blocking` so the CPU-bound pipeline runs
      on a blocking thread; the workflow function awaits the
      JoinHandle.
- [x] 3.3 Move the `SurrealdbSink::create` call to async (it now
      requires `.await`).
- [x] 3.4 Adjust `WorkflowOutcome` and error mapping if the change
      surfaces new error variants (e.g. `JoinError` from
      `spawn_blocking`).

## 4. MCP tools: async dispatch

- [x] 4.1 Convert `ServerState::with_db` to async; closure parameter
      becomes `FnOnce(...) -> impl Future<Output = ...>`. Tool
      bodies await via the closure.
- [x] 4.2 Convert each tool function in `tools.rs` (`get_symbol`,
      `find_at_location`, `list_*`, `search_*`, `get_workspace_overview`,
      `get_index_status`) to `async fn`. Internal callsites switch
      from sync `h.read.foo()` to `h.read.foo().await`.
- [x] 4.3 Remove `ServerState::bootstrap_blocking`. Add async
      `bootstrap` if needed by tests/orchestration; otherwise the
      orchestration's startup decision handles the equivalent
      logic inline.
- [x] 4.4 In `server.rs::run_tool`: drop `tokio::task::spawn_blocking`;
      simply `.await` the tool function.
- [x] 4.5 Lifecycle lock (`std::sync::RwLock<LifecycleState>`) reads
      must NOT hold guards across `.await`. Pattern: match on
      `&*guard`, copy needed fields out, drop guard, then await on
      the storage call. Audit every site.

## 5. MCP indexing orchestration

- [x] 5.1 Convert `start_background_indexing` to spawn a single
      `tokio::spawn`'d async task instead of `spawn_blocking`. The
      task awaits `index_workspace`.
- [x] 5.2 The notification-pump task is unchanged (still
      `tokio::spawn`).
- [x] 5.3 The synchronous `bootstrap_blocking` fast path is replaced
      by an `await`ed call inside the async task at startup.
- [x] 5.4 Update the `bootstrap_blocking`-tagged comments and doc
      links pointing at it.

## 6. CLI: top-level runtime wrapper

- [x] 6.1 `cmd_index::run` builds a `Builder::new_multi_thread`
      runtime and `block_on`s an `async fn run_async(...)` that
      contains the existing body, with `.await`s where storage
      calls used to be sync.
- [x] 6.2 `cmd_mcp::run` already builds a runtime; tool body
      simplifies to `.await` calls.
- [x] 6.3 `cmd_status` (if it touches `ReadDb`) gets the same
      block_on wrapper.

## 7. Tests + validation

- [x] 7.1 Convert `kenn-store` `db.rs` unit tests to `#[tokio::test]`
      where they call now-async methods. Verify all assertions
      still pass.
- [x] 7.2 Convert `kenn-mcp` `tests/end_to_end.rs` to `#[tokio::test]`
      (in-process tests that previously used `bootstrap_blocking`).
- [x] 7.3 Convert `kenn-mcp` `tests/lifecycle.rs` similarly. The
      `live_snapshot_bootstraps_to_ready` test calls the new async
      `bootstrap` (or whatever replaces `bootstrap_blocking`).
- [x] 7.4 New unit test: launch `kenn mcp` (subprocess) against a
      published snapshot and observe that NO blocking thread pool is
      consumed during a tool call. (Soft test; rely on correctness +
      `cargo test --workspace` for primary signal.)
- [x] 7.5 `cargo test --workspace` passes (no regressions).
- [x] 7.6 `cargo clippy --workspace --all-targets` clean (only
      pre-existing warnings remain).
- [x] 7.7 Manual smoke: run `kenn index` against the app
      workspace; wall-time matches pre-refactor numbers (no
      regression). Run `kenn mcp` and exercise a few tools; verify
      progress notifications still fire and lifecycle transitions
      still work.
