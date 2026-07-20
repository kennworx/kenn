## 1. Pipeline progress callback

- [x] 1.1 Define `ProgressEvent` enum in `kenn-indexer::pipeline`
      (`Started`, `Phase { ... }`, `Batch { ... }`, `PhaseEnd { ... }`,
      `Completed { ... }`).
- [x] 1.2 Add `progress: impl Fn(ProgressEvent) + Send + Sync` parameter
      to `run_pipeline`. Existing call sites in `cmd_index.rs` pass
      `|_| {}`.
- [x] 1.3 Emit events at the agreed points: start, each phase boundary,
      each batch flush, completion. Pull existing `BENCH` printlns
      out of the bench-only path and into events that the bench code
      can consume via the same callback.
- [x] 1.4 Update internal pipeline tests to assert callback is called
      with expected event shapes for a small fixture.

## 2. Optional FULLTEXT defer flag

- [x] 2.1 Add `SinkOptions { defer_fulltext: bool }` (default `false`)
      to `kenn-store::SurrealdbSink`. Add
      `SurrealdbSink::create_with_options(dir, opts)` constructor; keep
      `create(dir)` as a shorthand.
- [x] 2.2 Wire `defer_fulltext` into the sink's open path. When false:
      apply both `SCHEMA_SURQL` and `FULLTEXT_SURQL` at `begin_run`,
      no-op `end_run` for indexes (current behavior). When true:
      apply `SCHEMA_SURQL` only at `begin_run`, apply `FULLTEXT_SURQL`
      at `end_run`.
- [x] 2.3 Update `cmd_index.rs` to pass `SinkOptions::default()`
      (no behavior change). Leave a comment noting MCP can opt in.
- [x] 2.4 Test: round-trip creating a sink with each flag value, write
      a tiny batch, assert FULLTEXT index is queryable in both cases.

## 3. Snapshot freshness helper

- [x] 3.1 Add `kenn-store::lifecycle::decide_startup_state(&Store,
      &kenn_toml_config) -> StartupDecision` returning either
      `Decision::Skip { snapshot_id, indexed_at, ... }` or
      `Decision::Reindex { reason: &'static str }`.
- [x] 3.2 Implement using `compute_staleness_key` +
      `StalenessKey::matches`, honoring `staleness.git_aware_skip`.
      Conservatively return `Reindex` when staleness metadata cannot
      be read.
- [x] 3.3 Tests: Reindex when no live, Skip when key matches, Reindex
      when key differs, Reindex when staleness disabled in config.
- [x] 3.4 Refactor `cmd_index.rs` to call this helper rather than
      inline checks (verifying behavior is identical).

## 4. MCP server lifecycle state machine

- [x] 4.1 Define `LifecycleState` enum in `kenn-mcp::state`:
      `Indexing { started_at, progress: Option<ProgressSnapshot> }`,
      `Ready { read_db, snapshot_id, indexed_at, fallback_from_parent }`,
      `Failed { error, started_at, ended_at }`.
- [x] 4.2 Refactor `ServerState` to hold an
      `Arc<RwLock<LifecycleState>>` instead of an always-open
      `ReadDb`. Keep all existing accessors for back-compat where
      cheap; gate read-db access behind a `with_db_when_ready`
      helper that returns `INDEX_UNAVAILABLE` if not Ready.
- [x] 4.3 Add `INDEX_UNAVAILABLE` variant to `kenn-mcp::error::McpError`
      with appropriate JSON-RPC code mapping.

## 5. MCP startup orchestration

- [x] 5.1 In `serve_stdio`, after constructing `ServerState`:
      - Call `decide_startup_state` (from §3).
      - If `Skip`: open `ReadDb` from `live/`, set
        `LifecycleState::Ready`.
      - If `Reindex`: set `LifecycleState::Indexing { started_at: now,
        progress: None }`, spawn the background task (§5.2).
- [x] 5.2 Background indexing task: `tokio::task::spawn_blocking` that
      calls `run_pipeline` with a progress callback that:
      (a) updates `LifecycleState::Indexing.progress`,
      (b) pushes a `ProgressEvent` into an
      `mpsc::UnboundedSender<ProgressEvent>` shared with the runtime
      task in §5.3.
- [x] 5.3 Notification-pump task: a `tokio::spawn`'d async task that
      receives `ProgressEvent`s and emits rmcp
      `notifications/message` info-level logs. Drop policy if the
      receiver is gone (transport closed) is non-fatal.
- [x] 5.4 On pipeline completion: if `Ok`, open `ReadDb` and write
      `LifecycleState::Ready`. If `Err`, write
      `LifecycleState::Failed { error: e.to_string(), ... }`.
- [x] 5.5 Bind stdio at the same time as (or before) starting the
      background task. Confirm `tools/list` and `tools/call
      get_index_status` work during indexing.

## 6. Tool dispatch state-aware

- [x] 6.1 Add `state-check` shim at the top of every tool except
      `get_index_status`:
      ```
      let state = self.state.lifecycle.read().await;
      let LifecycleState::Ready { .. } = state.deref() else {
          return Err(McpError::index_unavailable(state_kind, ...));
      };
      ```
      via a small helper to avoid copy-paste.
- [x] 6.2 Update `tools::get_index_status` to read `LifecycleState`
      directly and produce the new payload shape:
      `state` field + state-specific fields (`progress` when
      indexing, `error` when failed, `snapshot_id` etc. when ready).
- [x] 6.3 Extend `IndexStatus` struct in `kenn-mcp::types` with
      `state` (string), `progress` (optional struct), `error`
      (optional string). All currently-required fields become
      optional (only present when applicable).
- [x] 6.4 Update the JsonSchema for `IndexStatus` (rmcp uses it for
      tool result schema generation). Verify rmcp's tool-list output
      still validates.

## 7. Tests + validation

- [x] 7.1 New integration test: launch `kenn mcp` in a tmp workspace
      with no `.kenn/`. Assert stdio binds (initialize handshake
      completes), `get_index_status` returns `state: "indexing"`,
      `search_symbols` returns `INDEX_UNAVAILABLE`. After indexing
      completes, assert `state: "ready"` and tools work.
- [x] 7.2 New integration test: launch `kenn mcp` in a workspace with
      a fresh `live/`. Assert the server transitions directly to
      `Ready` without re-indexing (verify by snapshot mtime or by
      checking no new snapshot directory is created).
- [x] 7.3 New integration test: simulate pipeline failure (e.g.
      missing kenn-dotnet binary, language config that points at a
      nonexistent .sln). Assert `state: "failed"`, error message
      includes the cause, and the state stays `failed` across
      subsequent calls.
- [x] 7.4 `cargo test --workspace` passes (no regressions).
- [x] 7.5 `cargo clippy --workspace --all-targets` clean (only
      pre-existing warnings remain).
- [x] 7.6 Manual smoke: run `kenn mcp` against the app workspace
      from a fresh state, confirm progress notifications stream to
      stderr, agent calls `get_index_status` periodically and sees
      progress updating, indexing completes and tools become
      available.
