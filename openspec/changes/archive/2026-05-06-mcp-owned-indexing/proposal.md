## Why

Today the MCP server (`kenn mcp`) requires a published snapshot at
`.kenn/live/` before it can bind stdio — there's no story for "agent
launched the MCP, no index exists yet." The only path is for the user to
run `kenn index` first and wait.

When the agent IS the user (autonomous code-tasks, IDE integrations),
that two-step is friction. Agents expect a single command that comes
up immediately, reports its own progress, and answers tool calls when
ready. Treating MCP as the orchestrator of its own indexing makes the
agent flow first-class.

## What Changes

- `kenn mcp <ws>` binds stdio **immediately**, before any indexing.
  The server is reachable from the moment the process starts.
- On startup the server inspects `.kenn/live/`:
  - If a fresh, valid snapshot exists → transition to **Ready**, serve.
  - If missing or stale (per existing `index-store-staleness` checks) →
    transition to **Indexing**, kick off `run_pipeline` on a background
    tokio task.
- Lifecycle state machine inside `ServerState`:
  `Indexing → Ready` on success, `Indexing → Failed` on error.
  No automatic retries; reindex triggers are out of scope here
  (file-watcher / incremental updates are deferred).
- Tool routing is state-aware:
  - `get_index_status` works in **every** state and returns the lifecycle
    status, including progress fields when indexing.
  - All other tools return a new `INDEX_UNAVAILABLE` error code while
    the server is in `Indexing` or `Failed`. This is the MCP contract;
    agents handle it (retry, mark not-ready, etc.).
- Pipeline-side: `run_pipeline` accepts an optional progress callback.
  Each pipeline phase emits events; MCP wires the callback to rmcp
  `notifications/message` (info-level) so agents see live progress.
- Optional FULLTEXT defer: `SurrealdbSink` gains a `defer_fulltext`
  option (default off). With it on, the two BM25 indexes are built in a
  separate pass at end-of-run instead of incrementally. Default keeps
  current `kenn index` behavior unchanged. MCP enables it for itself
  in a follow-up change once phase-gated tooling lands.

## Capabilities

### New Capabilities

- `mcp-orchestrated-indexing`: defines how MCP owns its indexing
  lifecycle — startup snapshot check, background pipeline invocation,
  state machine, progress callback contract, and the optional
  FULLTEXT-defer flag.

### Modified Capabilities

- `mcp-server`: the MCP read API. Server now starts without a
  pre-existing snapshot; all tool calls except `get_index_status`
  return `INDEX_UNAVAILABLE` while not Ready. `get_index_status`
  response gains lifecycle-state fields.

## Impact

- **Code**:
  - `crates/kenn-mcp/src/server.rs`, `state.rs` — new state-machine
    lifecycle; tools dispatch through state check.
  - `crates/kenn-mcp/src/tools.rs` — `get_index_status` works in any
    state; other tools fail-fast with `INDEX_UNAVAILABLE`.
  - `crates/kenn-mcp/src/lib.rs` — `IndexStatus` shape extended with
    lifecycle state + progress.
  - `crates/kenn-indexer/src/pipeline.rs` — `run_pipeline` accepts an
    optional `progress: impl Fn(ProgressEvent)` callback. Existing
    callers pass a no-op closure; behavior unchanged.
  - `crates/kenn-store/src/db.rs` — `SurrealdbSink::create` gains a
    `SinkOptions { defer_fulltext: bool }` parameter (default false).
- **APIs**: MCP tool error vocabulary gains `INDEX_UNAVAILABLE`. The
  `get_index_status` response payload gains `state` (string),
  `progress` (optional struct), and `error` (optional string) fields.
- **Schema**: unchanged. The optional FULLTEXT split was already landed
  via separate constants (`SCHEMA_SURQL` + `FULLTEXT_SURQL`); the flag
  selects whether the sink applies them together or sequentially.
- **Performance**: no change to `kenn index` (default-off flag). For
  `kenn mcp` cold-start, time-to-active drops from "wait for full
  index then bind stdio" to "bind immediately". Time-to-first-useful-
  call equals streaming + B-tree-index time (~38s on the app
  workload) in a follow-up that turns the FULLTEXT-defer flag on.
- **Lifecycle**: snapshot lifecycle (`live` swap, GC, rollback) is
  unchanged. MCP-driven indexing reuses the existing
  `building/ → snapshots/<ts>/` flow.
- **Out of scope**: file-watcher reindex, incremental updates, phase-
  gated tooling (FULLTEXT-aware fallback for `search_*` tools), `kenn
  init` UX redesign. Each of these is a separate proposal.
