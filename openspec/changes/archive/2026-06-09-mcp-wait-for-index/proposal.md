## Why

On cold start an agent's first orientation calls (`get_index_status`,
`get_workspace_overview`) can race the indexing pipeline. When the server is
still `Indexing`, data tools fail fast with "retry once ready" — but the agent
has no way to *wait*, so it polls awkwardly or gives up. Worse, when the server
serves an **empty `Ready` snapshot** (a stale/partial snapshot published before
the workspace settled), the agent sees `symbol_count: 0` / `not-initialized`,
concludes "the index isn't built," and abandons kenn for manual exploration —
even though a populated index is moments away or already expected.

## What Changes

- Add a **`wait_for_index`** MCP tool: blocks until the index is *settled*
  (`Ready` with no in-flight reindex) or `Failed`, up to a caller-supplied
  `timeout_ms` (sane default, hard cap). Returns the same status shape as
  `get_index_status` plus a `timed_out` flag. The existing data tools stay
  fail-fast; this is the one explicitly-blocking, opt-in tool.
- **Harden cold-start** so the server does not present an empty/stale snapshot
  as a settled `Ready` state when a populated index is expected: on startup,
  an empty live snapshot under a language-enabled config is re-indexed (server
  stays `Indexing`) rather than served as an empty `Ready`. A genuinely empty
  workspace (no `kenn.toml`, or no languages enabled) still settles to `Ready`
  with the existing config-hint — no reindex loop.
- **Guide the agent**: the `Indexing` / reindex-in-progress conditions point at
  `wait_for_index`, so an agent that hits a not-yet-ready index is told to wait
  rather than to interpret an empty result.

## Capabilities

### New Capabilities
<!-- none — both changes modify existing capabilities -->

### Modified Capabilities
- `mcp-server`: adds the `wait_for_index` tool to the tool surface (a blocking
  companion to the non-blocking `get_index_status`), and points not-ready
  conditions at it.
- `mcp-orchestrated-indexing`: cold-start must not settle on an empty/stale
  snapshot as `Ready` when the config expects symbols — re-index instead of
  serving empty; the wait/settle contract that `wait_for_index` observes is
  defined against the lifecycle this capability owns.

## Impact

- **Code:** `crates/kenn-mcp` — new tool in `tools/lifecycle.rs`, registration
  in the rmcp tool surface (`server.rs`) and `tools/mod.rs`; cold-start change
  in `indexing/orchestrate.rs` (startup decision). No store/schema changes.
- **API:** one new MCP tool; no breaking changes to existing tools. The
  `get_index_status` response shape is reused.
- **Behavior:** agents can block on indexing with a timeout; cold start no
  longer surfaces a misleading empty `Ready` for a configured workspace.
