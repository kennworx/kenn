## Context

`kenn mcp` is a thin reader over a published snapshot at `.kenn/live/`.
Today it can't start without one: `ServerState` opens a `ReadDb` at
construction time, and indexing is a separate workflow run via
`kenn index`. For autonomous-agent use cases — where the agent IS the
launcher — the user-visible flow is "agent starts MCP, MCP works." The
two-step "index then serve" is friction that breaks that promise.

This change makes MCP own its indexing: the server binds stdio
immediately, runs indexing in-process if needed, and answers tool calls
when ready. The lifecycle becomes a state machine the agent can observe
via `get_index_status` and the standard MCP `notifications/message`
stream.

Constraints we work within:
- Existing snapshot lifecycle (`building/`, `snapshots/<ts>/`, `live`)
  stays. MCP-driven indexing writes through the same path.
- Existing staleness machinery (`compute_staleness_key`,
  `git_aware_skip` config) is reusable.
- `run_pipeline` is the canonical indexing entry point and is already
  called from `cmd_index.rs`. It must stay reusable from there.
- rmcp uses tokio multi-thread runtime by default; pipeline calls are
  blocking I/O + CPU. Background work runs on a tokio task spawned with
  `spawn_blocking` to avoid starving the IO scheduler.

## Goals / Non-Goals

**Goals**

- MCP server binds stdio with zero indexing latency.
- `get_index_status` is callable from the moment the server is up,
  reflecting current lifecycle state.
- Other tools fail fast with a typed error during indexing — agents see
  a clear contract, not silent hangs.
- Pipeline emits progress events through a callback; MCP forwards them
  to agents via standard MCP logging notifications.
- FULLTEXT split (already implemented) becomes opt-in. Default behavior
  of `kenn index` does not change.

**Non-Goals**

- Phase-gated tools (B-tree-ready vs FULLTEXT-ready). Search tools
  fall back to non-BM25 scans only when phase-gated indexing turns on.
  That's a follow-up.
- File-watcher / incremental reindex. Triggers and update strategies
  are a separate research thread.
- `kenn init` UX. Future change.
- Automatic retry on indexing failure. `Failed` is terminal until
  process restart.
- Cross-process coordination. If two `kenn` processes try to write
  `building/` simultaneously, the existing `index.lock` handles it.

## Decisions

### D1. State machine inside `ServerState`

`ServerState` holds an `Arc<RwLock<LifecycleState>>` instead of an
always-open `ReadDb`. Tools acquire a read lock at the top of their
dispatch and pattern-match on the state.

```
enum LifecycleState {
    Indexing { started_at, progress: Option<ProgressSnapshot> },
    Ready { read_db, snapshot_id, indexed_at, fallback_from_parent },
    Failed { error: String, started_at, ended_at },
}
```

**Why a single state machine** (rather than e.g. `Option<ReadDb>` plus a
separate `IndexJob` field): one source of truth for "is the server
ready", easy to reason about transitions, easy to test.

**Why `RwLock`** (not `Mutex`): tools take read locks; the only writer
is the background indexing task that flips `Indexing → Ready/Failed`
once. Read-heavy.

Alternative considered: `arc-swap::ArcSwap<LifecycleState>`. Lock-free
reads, atomic writes. Marginal perf benefit at our QPS; std `RwLock` is
simpler.

### D2. Background indexing on `tokio::task::spawn_blocking`

`run_pipeline` is synchronous and CPU-heavy. Spawning it on the rmcp
runtime's worker pool would pin a worker for tens of seconds. Use
`spawn_blocking` so it lands on tokio's blocking thread pool (default
512 threads, plenty of headroom for one indexing job).

The blocking task signals completion by acquiring the `RwLock` write
guard and replacing `Indexing` with `Ready`/`Failed`.

Alternative considered: spawn the indexer as a subprocess (re-exec
`kenn index` internally). Cleaner isolation; failure modes are simpler.
Rejected because progress reporting becomes IPC, the rmcp side has to
parse stderr, and we lose the in-process callback. Subprocess can be
reconsidered if MCP indexing reliability becomes an issue.

### D3. Progress callback signature

`run_pipeline` gains a parameter:

```rust
progress: impl Fn(ProgressEvent) + Send + Sync + 'static
```

`ProgressEvent` enum carries phase + counters:

```rust
enum ProgressEvent {
    Started,
    Phase { phase: Phase, started_at_ms: u128 },
    Batch { records: u64, files: u64, symbols: u64 },
    PhaseEnd { phase: Phase, elapsed_ms: u128 },
    Completed { snapshot_id: String, total_ms: u128 },
}
```

CLI callers (`cmd_index.rs`) pass `|_| {}` — no behavior change. MCP
passes a closure that:
1. Updates `LifecycleState::Indexing.progress` (held in `Arc<RwLock>`).
2. Emits an rmcp `notifications/message` info-level log.

**Why a callback, not a channel**: the consumer is on a different
runtime task; a callback is the simplest contract. The callback can
internally route to a channel if needed. We keep the callback type
parametric (impl Fn) to avoid trait-object overhead on a per-batch path.

### D4. `INDEX_UNAVAILABLE` error code

New variant in `McpError::ErrorCode`. JSON-RPC error code chosen to be
distinct from existing codes; the JSON-RPC reserved range is -32768 to
-32099, so we use a server-error-band code.

Tool dispatch becomes:

```rust
match state.read().await.deref() {
    LifecycleState::Indexing { .. } | LifecycleState::Failed { .. } =>
        return Err(McpError::index_unavailable(...)),
    LifecycleState::Ready { read_db, .. } => {
        // existing tool body, with read_db available
    }
}
```

`get_index_status` is the only tool that doesn't dispatch through this
gate — it reads the state directly.

### D5. Snapshot freshness check

Reuse `compute_staleness_key` + `StalenessKey::matches` from
`kenn-store::staleness`. On startup:

1. Open `Store::open(workspace.kenn_dir())`.
2. If no `live/` symlink exists → `Indexing`.
3. Compute current `StalenessKey`. If `git_aware_skip` config is true
   and the key matches the snapshot's stored key → `Ready` (skip
   indexing). Otherwise → `Indexing`.
4. If reading the snapshot's stored key fails → `Indexing` (treat as
   stale, conservative).

The staleness machinery is already exercised by `cmd_index.rs`. We
extract its logic into a small `decide_startup_state` helper in
`kenn-store::lifecycle` so MCP and CLI agree on what "stale" means.

### D6. `SinkOptions { defer_fulltext: bool }` (default false)

`SurrealdbSink::create` keeps its current 1-arg signature for source
compatibility; we add `SurrealdbSink::create_with_options(dir, opts)`.
The `create` shorthand calls `create_with_options(dir, default)`.

The pipeline → sink → schema-application path is unchanged for the
default case. With `defer_fulltext: true`, the sink loads
`SCHEMA_SURQL` (no FULLTEXT) at `begin_run` and applies `FULLTEXT_SURQL`
at `end_run`. This is exactly what the FULLTEXT split already does;
this change just gates the deferral on the flag instead of unconditional.

MCP does NOT enable the flag in this proposal. It's left available for
the follow-up phase-gated-tools change to switch on.

## Risks / Trade-offs

- **Risk**: Pipeline failure leaves the MCP in `Failed` until process
  restart. → Mitigation: error message includes guidance to re-launch
  or run `kenn index` manually. Reindex-from-MCP is a future tool.
- **Risk**: Background indexing on tokio's blocking pool ties up one
  thread for tens of seconds. → Mitigation: blocking pool is large
  (512 threads default), and rmcp IO threads are separate. Confirmed
  not a contention problem at our scale.
- **Risk**: Snapshot freshness false-negative — staleness check reports
  fresh, but data on disk is actually inconsistent (e.g. partial write
  recovered after a crash). → Mitigation: existing
  `lifecycle::publish` flow uses an atomic symlink swap; partial states
  don't escape `building/`. The check is best-effort; on doubt, agents
  can manually trigger reindex (post-foundation).
- **Risk**: Progress callback called from a different thread than rmcp
  runtime; sending notifications could block. → Mitigation: callback
  pushes events into an `mpsc::UnboundedSender`; an async task on the
  MCP runtime drains the receiver and sends notifications. Decouples
  the indexing thread from rmcp's IO.
- **Trade-off**: We keep `kenn index` as a separate command instead of
  folding it into MCP-only. Two-step is still possible (init flow may
  use it), and tests/CI use it heavily. Acceptable.

## Migration Plan

No data migration. No on-disk format changes. Existing snapshots remain
readable.

Rollout:
1. Land this change.
2. `kenn index` behavior unchanged (regression tests prove it).
3. `kenn mcp` becomes self-bootstrapping. Existing users of `kenn mcp`
   (which require pre-built `live/`) keep working — fresh snapshots
   pass the `Ready` check immediately.

Rollback strategy: revert. No persistent state changes to undo.

## Open Questions

None blocking. Three deferred to follow-up changes:

1. **Phase-gated tools / FULLTEXT fallback**: when MCP enables
   `defer_fulltext`, search tools need a fallback path
   (`name CONTAINS '<q>'`) until BM25 is built. Out of scope here;
   the foundation just makes the flag plumbing work.
2. **File-watcher / reindex triggers**: post-foundation. Needs
   research on incremental vs full-replace.
3. **Manual `reindex` MCP tool**: would let agents trigger a fresh
   indexing run without restarting the process. Useful but not
   foundation-blocking.
