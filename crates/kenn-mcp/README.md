# kenn-mcp

MCP server for kenn snapshots. Exposes 23 tools that AI agents (and any
other MCP client) can call to navigate, search, and introspect an
indexed workspace, plus a **knowledge layer** for recording and reusing
provenance-tracked findings.

## Running

The server runs as a subcommand of the `kenn` CLI:

```sh
# Index the workspace first (one-time, then re-runs as needed).
kenn index

# Speak MCP over stdio. Wire this into your agent's MCP configuration.
kenn mcp
```

There is no separate binary; `kenn mcp` is the supported entry point.

## Tool catalog

| Tool | Returns | Default `include_tests` |
|---|---|---|
| **search** | | |
| `find_symbol` | `ListResponse<SymbolRef + match_kind>` | `false` |
| `search_symbols` | `ListResponse<SymbolRef + name_score/doc_score/score>` | `false` |
| `get_symbol` | `SingleResponse<SymbolDetail>` | n/a |
| `find_at_location` | `ListResponse<SymbolRef>` (smallest-enclosing first) | n/a |
| **navigate** | | |
| `list_callers` | `ListResponse<SymbolRef>` | `false` |
| `list_callees` | `ListResponse<SymbolRef>` | `false` |
| `list_implementers` | `ListResponse<SymbolRef>` | `false` |
| `list_overrides` | `ListResponse<SymbolRef>` | `false` |
| `list_usages` | `ListResponse<SymbolRef + via_edge_kind>` | `false` |
| `list_correspondences` | `ListResponse<SymbolRef>` | `false` |
| **scope** | | |
| `list_in_scope` | `ListResponse<SymbolRef>` (direct children, v1) | `false` |
| `list_imports` | `ListResponse<SymbolRef + direction?>` | `false` |
| `list_module_files` | `ListResponse<FileRef>` | n/a |
| **meta** | | |
| `get_workspace_overview` | `SingleResponse<WorkspaceInfo>` | n/a |
| `get_index_status` | `SingleResponse<IndexStatus>` | n/a |
| **knowledge layer** | | |
| `semantic_search` | `SemanticSearchResponse` (code + findings groups) | n/a |
| `get_source` | `SingleResponse<SourceView>` | n/a |
| `get_finding` | `SingleResponse<FindingView>` | n/a |
| `search_findings` | `ListResponse<RankedFindingView>` (`stale` per row) | n/a |
| `store_finding` | `StoreFindingResponse` (`{ id, similar }`) | n/a |
| `merge_findings` | `SingleResponse<String>` (new finding id) | n/a |
| `find_predecessors` | `ListResponse<String>` (provenance ids) | n/a |
| `find_successors` | `ListResponse<String>` (derived finding ids) | n/a |

The code-graph tools are read-only and never mutate the index;
reindexing is CLI-only via `kenn index`. The knowledge-layer write
tools (`store_finding`, `merge_findings`) commit to the durable
findings store — a workspace-resident dataset independent of the
per-index-run snapshot.

## Knowledge layer

The findings store is the server's **shared memory**: a durable,
append-only record of agent-derived conclusions, each carrying
provenance (`parent_ids` spanning code-graph nodes and earlier
findings). It carries knowledge across tasks, sessions, and
orchestration stages — each task makes the next cheaper.

The server stays **dumb primitives**: it holds no model and performs no
task analysis. It reads the graph, reads and writes findings, and walks
the derivation DAG. All reasoning — what to investigate, how to slice a
task, when to record a conclusion — is the calling agent's.

The agent guide lives at [`assets/kenn-agent.md`](assets/kenn-agent.md) — the
code graph plus the findings/directives knowledge layer (search findings before
re-investigating, store at a stable conclusion, recall directives before editing,
squeeze before commit). It is injected into the session as the MCP server's
`instructions` (and printed by `kenn instructions`).

### Subagent-as-extractor dispatch

For a task that decomposes into genuinely independent sub-investigations,
a main agent runs **orient → slice → fan-out → record → synthesize**:

1. **Orient** — `semantic_search` and graph reads to find anchors.
2. **Slice** — decide how many subagents and what each investigates.
3. **Fan-out** — dispatch general-purpose subagents in one message.
4. **Record** — each subagent investigates its slice through the MCP
   surface and calls `store_finding`, returning the finding ids.
5. **Synthesize** — the main agent collects the returned ids, reads the
   findings, and optionally `merge_findings` into a higher-level result.

Coordination is through the **findings store and the returned finding
ids** — not ad-hoc file passing. Fan-out is worthwhile only when the
sub-investigations are genuinely independent; a single-anchor lookup
needs no subagents. Slicing quality is an agent-prompt concern — the
server offers no planning or work-slicing tool by design.

## Response envelopes

Every list/search tool returns:

```ts
type ListResponse<T> = {
  items:  T[]              // [] when count_only=true OR no matches
  total:  number           // matches regardless of pagination
  next:   string | null    // opaque cursor; null when exhausted
}
```

Single-result tools return:

```ts
type SingleResponse<T> = {
  item:        T | null
  found:       boolean
  not_found?:  { parent_id?: string, parent_kind?: Kind }   // get_symbol only
}
```

## Cursor pagination

Cursors are opaque, base64-encoded, and bound to the live snapshot:

- list cursors carry `(snapshot_id[6], last_short_id[4])`
- search cursors additionally carry `(last_bm25_score[4 LE f32])`

Pass the returned `next` verbatim as `pagination.cursor` for the next page.
If the index rotated mid-pagination, the server returns a `STALE_CURSOR`
JSON-RPC error with `data.expected_snapshot_id` and `data.current_snapshot_id`
populated. The agent's correct response is to restart from page 1; the
analysis based on the previous snapshot would already be inconsistent
with the new one.

## Error model

| Condition | Mechanism |
|---|---|
| `get_symbol` for nonexistent id | `SingleResponse { found: false, not_found }` |
| `list_*` no matches | `ListResponse { items: [], total: 0, next: null }` |
| stale cursor | JSON-RPC error `STALE_CURSOR` |
| `.kenn/live/` missing | JSON-RPC error `INDEX_UNAVAILABLE` |
| invalid id / unparseable cursor | JSON-RPC error `INVALID_INPUT` |
| db query failure | JSON-RPC error `INTERNAL_ERROR` |

## Worktree fallback

When this workspace is a `git worktree`-style linked checkout that hasn't
been indexed yet, the server falls back to the main worktree's snapshot
in read-only mode. `get_index_status.fallback_from_parent_worktree` is
`true` in this case so the agent can label results accordingly.

## Architecture

The MCP layer is a thin ServerHandler over `kenn-store::db::ReadDb`.
Per-tool query methods compose generic edge traversals (`list_inbound` /
`list_outbound`), BM25 search on `symbols.name` / `symbol_docs.documentation`,
and direct fetches keyed by short_id. See [`docs/kenn/store-architecture.md`](../../docs/kenn/store-architecture.md)
for the snapshot lifecycle the server reads from.

## Drift notes

- **`pub_id` is no longer unique.** As of `wire-pkg-and-stubs`, multiple
  rows in `symbols` may share the same `pub_id` (e.g. same descriptor
  across different package versions). `get_symbol(id)` returns the
  first match ordered by `short_id`; disambiguation by `pkg` is the
  caller's responsibility until the MCP redesign lands.

## v1 deferrals

The proposal lists items below as deferred until empirical needs surface:

- File-watcher–driven `is_stale` (always `false` in v1; reindex is explicit)
- `list_usages` cursor pagination across heterogeneous edge kinds (current
  behavior: first-page-per-kind unioned to `limit`)
- Recursive `list_in_scope` (current behavior: direct children only)
- `not_found` parent-hint suggestions (the shape is preserved on the wire
  but `parent_id` / `parent_kind` are empty in v1)
- MCPB packaging (after MVP)
