## Why

A developer editing files all day must remember to call the `reindex` tool
(or run `kenn index`) to keep the MCP-served snapshot current; until they
do, the agent silently queries stale data. The original plan for closing
this gap was a `kenn watch` subprocess that called the CLI on a debounce —
but with `mcp-background-reindex` now shipped, the MCP server already
owns the reindex trigger, the hot-reload swap, and the progress channel.
Folding the watcher into the MCP server collapses three moving parts into
one and keeps the agent-visible surface a single tool call.

The watcher itself is dumb on purpose: it only *triggers* a reindex.
Whether real work happens is still decided by the staleness key and the
one-writer flock, so a no-op edit cycle stays free.

## What Changes

- Add an in-process file watcher to `kenn mcp`, exposed via two tools —
  `watch_start` (idempotent) and `watch_stop` — and surfaced in
  `get_index_status`
- Watcher pipeline: `notify` events → filter (source-language extensions,
  workspace excludes, nested-worktree exclusion) → debounce
  (`mcp.watch_debounce_ms`, default 30 s) → `spawn_background_reindex`
- Snapshot poll task emits an MCP `notifications/message`
  (`event: "snapshot_swapped"`) on every successful `ArcSwap` swap, so
  the agent is told the data is fresh regardless of who triggered the run
  (watcher, `reindex` tool, external `kenn index`)
- New `[mcp]` config section with `watch_on` (boot the watcher
  implicitly) and `watch_debounce_ms`; the legacy `staleness.file_watcher`
  and `staleness.file_watcher_debounce_ms` fields are removed (no
  production users — the wiring never shipped)
- Watcher extension set derives from `kenn-indexer`'s language registry —
  single source of truth, no parallel whitelist to drift

## Capabilities

### Modified Capabilities

- `mcp-orchestrated-indexing`: gains the in-process file watcher
  (tools + boot-time `watch_on`) and the snapshot-swap notification
  emitted by the poll task on every reader swap.

### Removed Capabilities

- `index-store-staleness`: the unimplemented "file-watcher staleness
  signal" requirement is removed. The capability shipped explicit
  invocation and the git-aware skip; the third signal moves out of
  staleness's territory entirely and reappears as an MCP feature.

## Impact

- New dependency: `notify` (cross-platform filesystem events), pulled
  into `kenn-mcp` unconditionally — the watcher is always compilable;
  whether it *runs* is decided at runtime by `watch_on` or a
  `watch_start` call
- `kenn-config` gains `McpConfig { watch_on: bool, watch_debounce_ms:
  u64 }`; `kenn init`'s starter `kenn.toml` surfaces the new `[mcp]`
  section
- `kenn-config::StalenessConfig` loses `file_watcher` and
  `file_watcher_debounce_ms`
- No new CLI subcommand; `kenn watch` is not added
- Tests use `tokio::time::pause` / `advance` for deterministic debounce
  timing — no `Clock` trait, no new test crate
