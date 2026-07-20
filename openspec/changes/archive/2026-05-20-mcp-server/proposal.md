## Why

Three foundational proposals are in place: `scip-indexing-pipeline` (producer), `indexed-store-and-lifecycle` (storage), `source-data-model` (schema). The data is producible, storable, and shaped — but no consumer exists. This proposal defines the **read API** that AI agents (and other clients) use to query the indexed code structure.

The read surface is shaped by two empirical inputs:
- `scratch/mcp-design/transcripts.md`: 5 realistic agent transcripts spanning navigation, debugging, refactoring, dependency analysis, and type-flow understanding across 5 languages.
- `scratch/mcp-design/query-probe.md`: 24 query shapes measured against real the C# spike data, with latency tiering and capability-vs-feasibility mapping.

Combined with the principle "we are not an IDE — we provide semantic structure; agents read source for syntactic detail" the result is a **15-tool surface** with a uniform response envelope and opaque snapshot-versioned pagination.

## What Changes

- Define **15 MCP tools** organized as: 4 search (search_symbols, search_by_intent, get_symbol, find_at_location), 6 navigate (list_callers, list_callees, list_implementers, list_overrides, list_usages, list_correspondences), 3 scope (list_in_scope, list_imports, list_module_files), 2 meta (get_workspace_overview, get_index_status).
- Define the **uniform response envelope**: `ListResponse<T> { items, total, next }` for list/search tools and `SingleResponse<T> { item, found, not_found? }` for single-result tools.
- Define **count_only mode** on every list/search tool: short-circuits the items materialization for cheap counts.
- Define **opaque cursor pagination** with snapshot_id embedded; mid-pagination reindex returns a `STALE_CURSOR` JSON-RPC error so agents restart cleanly.
- Define **filter parameters**: language[], kind[], package[], file[] (globs), include_external (default false), include_tests (per-tool default, documented).
- Specify the **MCP server runtime**: Rust binary using `rmcp`, stdio transport for v0, packaged as MCPB after MVP.
- All tools annotated `readOnlyHint = true`. No write surface in v1.

## Capabilities

### New Capabilities

- `mcp-server`: the read API for code structure data. Defines tool surface, response envelopes, pagination, error model, and the runtime shape (Rust + rmcp + stdio for v0). Producer-agnostic above the line; consumers below see only this contract.

### Modified Capabilities

None.

## Impact

- **Closes the vertical slice**. Producer + storage + schema + read API gives a complete end-to-end system. Implementations of all four proposals together ship the MVP.
- **Becomes the agent contract**. Public IDs, location format, response envelopes, error codes are stable across server upgrades. Agent context can hold IDs across sessions.
- **No mutation surface in v1**. Reindex, init, rollback are CLI-only (per `indexed-store-and-lifecycle/index-store-cli`). The MCP layer is read-only — drastically simplifies the security review and the tool surface.
- **Performance profile is set by the schema and the store**. This proposal does not introduce new query patterns; it surfaces the ones validated in the query-probe at the latencies measured (75% under 10 ms, 17% in the 10-100 ms band).

## Scope

**In scope:**
- 15 read tools, their parameters, response shapes, semantics.
- Filter object structure.
- Pagination contract (cursor format, snapshot versioning, staleness handling).
- count_only mode.
- Error model (JSON-RPC errors for STALE_CURSOR; envelopes for found/not-found).
- Runtime: Rust + `rmcp` SDK, stdio transport, single workspace per server instance.
- Tool annotations (readOnlyHint).
- Default filter values per tool, with rationale.

**Out of scope:**
- MCPB packaging (after MVP).
- Remote HTTP transport (we are local-only by design).
- MCP app widgets / interactive UI.
- Source-text retrieval (`get_code_snippet`-style tool) — agents use their own file-read tools.
- Write/mutate operations of any kind (reindex/init/rollback are CLI).
- Auth (local; no auth needed for v1).
- Auto-suggestion engine for `not_found` responses (parent_id + parent_kind hints land; smart suggestions deferred).
- Sampling, elicitation, MCP resources/prompts primitives — none needed in v1.
- Multi-workspace federation in a single MCP server (one server = one workspace).
