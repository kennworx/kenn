## Why

The `kenn mcp` server indexes once at startup and then serves that single
snapshot for the entire life of the process. A fresher snapshot — produced by
`kenn index` or the upcoming file-watcher — is ignored until the operator
restarts the server, and an agent mid-session has no way to refresh. The result
is silent staleness: the server keeps answering from a snapshot that no longer
reflects the code.

## What Changes

- **Snapshot hot-reload.** The MCP server detects when a newer snapshot has been
  published (`.kenn/live/` repointed) and atomically swaps its `Reader` to it —
  no restart. Today the `Reader` is pinned to the boot-time snapshot directory.
  A snapshot that fails to open leaves the current one in service.
- **Coordinated embedding refresh.** A snapshot produced by `kenn index` carries
  null embeddings (the indexer defers embedding to a post-index job). The server
  runs that job after cold-start *and* after every hot-reload swap, so
  vector-search coverage is never lost on a new snapshot. The job is coordinated
  cross-process — at most one embed run per snapshot across all instances — so
  N servers do not each re-run the expensive embedding inference.
- **New `reindex` tool.** An MCP tool that triggers an in-process reindex. From
  `Ready` it runs in the **background** — the server keeps serving the current
  snapshot throughout, then atomically swaps to the new one on completion. From
  `Failed` it acts as a **recovery retry** (`Failed → Indexing`), so a transient
  cold-start failure no longer forces a process restart.
- **`Ready` becomes non-terminal** — an internal lifecycle change; the MCP wire
  contract is unchanged (`get_index_status` still reports `state: "ready"`, so
  clients are unaffected). The one-directional `Indexing → Ready/Failed`
  lifecycle gains a `Ready → (reindexing) → Ready` cycle. Reads are never
  blocked during a background reindex; a failed background reindex leaves the
  prior `Ready` snapshot intact rather than entering `Failed`.
- **Real status reporting.** `get_index_status` returns the true `is_stale` and
  `reindex_in_progress` values (today hard-coded `false`) plus the in-flight
  reindex's progress snapshot, so an agent can see whether a refresh is running
  and how far along it is. The call stays cheap — staleness is evaluated on the
  background poll and cached, never computed (git work) on the call path.
- **GC safety.** A snapshot held open by a live `Reader` is pinned against
  `[lifecycle] gc_keep` collection so it cannot be deleted out from under a
  running server.
- **Multiple MCP instances on one workspace.** Each Claude session spawns its
  own `kenn mcp` process, so several servers share one `.kenn/` store. At most
  one reindex runs at a time: the `reindex` tool and cold-start reindex
  coordinate through the store's existing one-writer lock; instances that lose
  the race do not error — they wait and hot-reload the winner's snapshot.
  Snapshot-GC pins are **cross-process** — a snapshot held by *any* instance's
  reader is never collected.

## Capabilities

### New Capabilities

(None — this change extends existing MCP behavior.)

### Modified Capabilities

- `mcp-orchestrated-indexing`: the indexing lifecycle gains a non-terminal
  `Ready` state with background reindex and snapshot hot-reload; `get_index_status`
  surfaces real staleness and reindex-progress; a new `reindex` tool triggers an
  in-process refresh without blocking reads.

## Impact

- `crates/kenn-mcp`:
  - `state.rs` — `Ready` carries an optional in-flight reindex handle; `Ready` is
    non-terminal; `Reader` held behind an atomically swappable `Arc`.
  - `indexing.rs` — snapshot-watch + background-reindex task; atomic `Reader` swap
    on a newer published snapshot.
  - `tools.rs` / `server.rs` — new `reindex` tool; `get_index_status` returns real
    `is_stale` / `reindex_in_progress` / progress.
  - `types.rs` — `IndexStatus` carries reindex-progress fields.
- `kenn-store` — a snapshot opened by an MCP `Reader` must be pinned against GC
  (`[lifecycle] gc_keep`); the pin and the GC check are **cross-process** via a
  store-level `flock` reader registry that auto-releases on process death. The
  existing one-writer `index.lock` coalesces concurrent reindexes across
  instances; a new separate per-snapshot embed lock coordinates the embed job
  without blocking reindex.
- Complements the `file-watcher-reindex` change: that triggers `kenn index` on
  file edits; this change is what lets the running MCP server pick the result up.
- No new external dependencies.
