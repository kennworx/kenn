## Context

The MCP server is the **read consumer** of the indexed code structure data:

```
┌─────────────────────────┐    writes    ┌──────────────────┐
│ scip-indexing-pipeline  │ ───────────→ │ live snapshot in │
│ (producer)              │              │ .kenn/     │
└─────────────────────────┘              │ (indexed-store-  │
                                          │  and-lifecycle)  │
                                          └────────┬─────────┘
                                                   │ reads
                                                   ▼
┌──────────┐     MCP tools     ┌─────────────────────────────┐
│ AI agent │ ←───────────────→ │ mcp-server (this proposal)  │
└──────────┘                   └─────────────────────────────┘
                                            ▲
                                            │ schema definitions
                                            │
                                  ┌─────────┴───────────┐
                                  │ source-data-model   │
                                  │ (IDs, kinds, edges) │
                                  └─────────────────────┘
```

Empirical anchors:
- `query-probe.md`: 24 query shapes against 98k symbols / 600k edges. 75% < 10 ms, 17% in 10-100 ms, 8% slow or infeasible. Graph encoding is ~1000× faster than table encoding for multi-hop. Prefix search is ~20× slower than BM25. External symbols can blow up result sets.
- `transcripts.md`: 5 agent scenarios. Recurring needs: ranking + grouping, type info, enclosing-symbol context. Recurring waste: agents asking for snippets we now don't provide (redirect: agent's own file-read tool).

This design covers only the **API contract and runtime shape**. Storage, ingestion, and the schema are defined in their respective proposals.

## Goals / Non-Goals

**Goals:**
- 15-tool surface that covers the agent task shapes from the transcripts (navigation, debugging, refactoring, dependency analysis, type-flow understanding).
- Uniform response envelope reducing per-tool client code.
- Cursor pagination that survives reindex robustly (clean failure, not silent data drift).
- Read-only by spec: no security surface, no write tool design needed.
- Tight tool descriptions (each one lands in Claude's context every turn).
- Rust runtime sharing crates with the indexer/store.

**Non-Goals:**
- Source-text retrieval (`get_code_snippet`). Agents have their own file-read tools.
- Site-level call/use queries ("where exactly does A call B"). Agent reads the caller's source.
- Cursor-on-use-site lookup ("what symbol is at file:line:col" for non-defs). Stack-trace use case is covered by `find_at_location` returning the smallest enclosing symbol.
- IDE features: signature help, completion, hover-on-token, go-to-definition-from-position.
- Mutating operations. Reindex/init/rollback are CLI per `indexed-store-and-lifecycle`.
- Federated multi-workspace queries.
- Suggestion engine for renamed/moved symbols beyond the parent_id/parent_kind hint.

## Decisions

### D1. 15 tools, named for discoverability

Tools are named verb_object with consistent prefixes — `search_*`, `get_*`, `find_*`, `list_*`. Agents discover capabilities from names; one general `traverse_graph` tool would force agents to learn parameter trees instead of names.

```
SEARCH (4)
  search_symbols          — name-based BM25 search
  search_by_intent        — documentation-based BM25 search
  get_symbol              — exact lookup by public ID
  find_at_location        — what-contains-this-line stack-trace lookup

NAVIGATE (6)
  list_callers            — incoming calls
  list_callees            — outgoing calls
  list_implementers       — incoming implements (trait/interface → impl)
  list_overrides          — incoming overrides
  list_usages             — generalized "anything that uses this" — refactor scope
  list_correspondences    — cross-language / codegen equivalents

SCOPE (3)
  list_in_scope           — symbols inside a module/package (transitive default)
  list_imports            — module dependency edges (in/out/both)
  list_module_files       — physical files of a module

META (2)
  get_workspace_overview  — packages, languages, counts, snapshot id
  get_index_status        — staleness, fallback state, last reindex
```

**Rationale.** This count is the right side of the search-vs-execute pattern boundary (the skill's threshold is < ~15). At 15 we keep one-tool-per-action ergonomics; agents reach for the right tool by name without runtime discovery.

Alternatives considered:
- One generic `query_graph` tool with parameters. Rejected — agents discover by name; an opaque function with a parameter tree is harder to reach for. Tool descriptions are how Claude reads our API.
- Splitting `list_callers/callees/implementers/overrides/usages` into a single `list_related(direction, edge_kind)`. Rejected — same reason; named tools self-document.
- Adding `count_*` tools as separate operations. Rejected — `count_only: true` parameter on each list_* tool covers it without doubling the surface.

### D2. Uniform response envelope

```typescript
type ListResponse<T> = {
  items:  T[]              // [] when count_only=true OR no matches
  total:  number           // matching count, regardless of pagination
  next:   string | null    // opaque cursor; null when exhausted
}

type SingleResponse<T> = {
  item:        T | null
  found:       boolean
  not_found?:  { parent_id?: string, parent_kind?: Kind }   // get_symbol only
}
```

`truncated` boolean is omitted — `next != null` carries the same information.

**Rationale.**
- One envelope shape per tool category lets clients reuse parsing.
- `total` lets agents reason about result-set size (transcript T1 wanted "34 instantiation sites" reasoning).
- `next` is opaque to agents; encoded with snapshot_id (D5).
- `not_found` payload preserves enough context for the agent to retry: parent + kind let the agent issue `find_symbols(name=..., parent_id=..., kind=...)` if needed.

### D3. count_only short-circuits item materialization

Every list_* and search_* tool accepts `count_only: boolean` (default `false`). When true:
- Backend executes a `count` query, not a row-fetching query.
- Response: `{ items: [], total: N, next: null }`.

**Rationale.**
- Agent intent like "is this method used at all?" is satisfied by N>0; no need to materialize 47 SymbolRefs.
- Cheaper on the wire and on the database (count queries skip projection).

### D4. SymbolRef shape (lightweight) and SymbolDetail (full)

```typescript
type SymbolRef = {
  id:              string                 // public ID, e.g. "cs:Models.Order.Foo(string)"
  kind:            Kind
  language:        Language
  name:            string
  display_name:    string
  location:        string | null          // "./Models/Order.cs#42-50" or null
  package:         string                 // public ID of containing package
  module:          string                 // public ID of containing module
  args_arity:      number
  generic_arity:   number
  is_external:     boolean
  is_test:         boolean
  is_partial:      boolean
}

type SymbolDetail = SymbolRef & {
  signature_doc:   string                 // "" when not populated
  documentation:   string                 // "" when not populated
  defined_in:      SymbolRef | null
  primary_def:     { file: string, range: [number, number] }
  partial_defs?:   { file: string, range: [number, number] }[]   // when is_partial=true
}
```

`SymbolRef` is what every list/search tool returns. `SymbolDetail` is what `get_symbol` returns — adds docs and full def info.

The `via_edge_kind` tag is added on SymbolRef returned by `list_usages` so agents can group results by relation type.

### D5. Cursor format and stale-snapshot handling

Cursor is base64-encoded compact tuple, opaque to agents:

```
list_*    →  base64(snapshot_id || last_short_id)
                     6 bytes      4 bytes
                     ──────────  ────────
                     14 chars total

search_*  →  base64(snapshot_id || last_bm25_score || last_short_id)
                     6 bytes      4 bytes (f32)      4 bytes
                     ────────────────────────────────────────
                     20 chars total
```

`snapshot_id` is 6 hex chars derived from the snapshot's ISO-8601 timestamp (the directory name in `.kenn/snapshots/<iso>/`). Stable per snapshot, distinct across rebuilds.

When a tool is called with a cursor:
1. Decode cursor → `(snapshot_id, ...)`.
2. Compare to live snapshot's id.
3. **Match** → resume the iteration with `WHERE short_id > $last` (or BM25 equivalent for search).
4. **Mismatch** → return JSON-RPC error:
   ```json
   {
     "code": "STALE_CURSOR",
     "message": "Index was rebuilt during pagination",
     "data": { "expected_snapshot_id": "...", "current_snapshot_id": "..." }
   }
   ```

Agent retries by calling the same tool without a cursor.

**Rationale.**
- Snapshots are immutable; pagination within a snapshot is deterministic.
- Reindex is rare (minutes-long, single-developer per workspace). Mid-pagination reindex is an edge case; throwing is correct because the agent's analysis already used data that may diverge from the new state.
- Compact opaque format is small enough to be passed in MCP payloads cheaply.

### D6. Default filter values per tool (documented)

`include_external` defaults to `false` everywhere — externals (`System.String.Format` etc.) blow up result sets per `query-probe.md` finding 3.

`include_tests` defaults vary by tool:

| Tool | include_tests default | Rationale |
|---|---|---|
| `search_symbols` | `false` | name search usually wants production symbols |
| `search_by_intent` | `false` | same |
| `list_callers` | `true` | test code calling X is part of impact |
| `list_callees` | `true` | symmetry; you may be inspecting tests deliberately |
| `list_implementers` | `true` | test mocks count as implementations |
| `list_overrides` | `true` | test stubs count |
| `list_usages` | `true` | refactor scope MUST include test usages |
| `list_correspondences` | `true` | irrelevant; deterministic |
| `list_in_scope` | `true` | "everything in this scope" includes tests by default |
| `list_imports` | `true` | irrelevant; modules don't have a meaningful is_test |
| `list_module_files` | `true` | irrelevant; files have their own `is_test` field |

Each tool's description text in the MCP catalog states its default explicitly.

**Rationale.** No universal default fits both "search wants production" and "refactor wants tests." Per-tool defaults match each tool's most common use; agents override when they want the other behavior.

### D7. Filter values are arrays; not human-facing convenience

All multi-value filters use array shapes:

```typescript
type Filters = {
  language?:           Language[]
  kind?:               Kind[]
  package?:            string[]
  file?:               string[]            // globs
  include_external?:   boolean
  include_tests?:      boolean
}
```

A single value is wrapped in an array by the client.

**Rationale.** The MCP API is consumed by Claude (and other agents), not humans typing JSON by hand. Single-value convenience overloads (`kind: Kind | Kind[]`) double the surface for marginal benefit. Agents construct JSON from structured intent; arrays are uniform.

### D8. Tool annotations: readOnlyHint = true on all 15

MCP tool annotations declared:

```
readOnlyHint:    true     -- on all 15 tools
destructiveHint: false    -- (implied by readOnlyHint, set explicitly)
idempotentHint:  true     -- all reads are idempotent
openWorldHint:   false    -- no external network calls
```

**Rationale.** No write surface exists in v1. CLI handles init/index/rollback per `indexed-store-and-lifecycle`. Agents and hosts can short-circuit permission flows, batch reads aggressively, retry on errors.

### D9. find_at_location semantics: smallest enclosing first

```typescript
find_at_location({
  file:    string,            // workspace-relative path; exact match
  line:    number,
  kind?:   Kind[]             // default: all kinds whose def_range covers the line
}) → ListResponse<SymbolRef>
```

Returns symbols whose `def_range` contains `(line)`, ordered by specificity — smallest range first. The first row is the most specific (e.g., the method); subsequent rows are larger enclosing scopes (the class, then the module).

**Rationale.**
- Stack-trace debugging is the use case: "error at OrderService.cs:42, what's there?". The agent typically wants the containing method first.
- Multiple kinds at one line is normal (a method line is also inside its class and module). Letting the agent see the chain is useful (e.g., "method → class → module → package").
- The kind filter narrows to specific levels: `kind: ["method"]` returns only the smallest method; `kind: ["package"]` returns the package.

### D10. list_usages: tagged with via_edge_kind

```typescript
list_usages({
  id:           string,
  edge_kinds?:  EdgeKind[],   // default: ["calls", "type_use", "field_access", "instantiates"]
  op_filter?:   "read" | "write",   // applies only when "field_access" in edge_kinds
  filters?:     Filters,
  pagination?:  Pagination,
  count_only?:  boolean,
})
→ ListResponse<SymbolRef & { via_edge_kind: EdgeKind }>
```

Each result row tagged with the edge type that matched. Agents can group: "8 callers, 12 type uses, 3 instantiations."

**Rationale.**
- Refactor scope (transcript T1 — adding a column to Order) needs the union of multiple edge kinds, not just calls.
- Agents reason about "how many distinct kinds of usage" matters; tagging surfaces this without an extra query.
- Default edge_kinds covers the common refactor-scope set; specialized callers (e.g., generic_constraint) opt in.

### D11. list_imports direction parameter

```typescript
list_imports({
  id:           string,                    // a module/package
  direction:    "outbound" | "inbound" | "both",
  kind?:        ("explicit" | "re_export")[],
  filters?:     Filters,
  pagination?:  Pagination,
  count_only?:  boolean,
})
→ ListResponse<SymbolRef & { direction?: "outbound" | "inbound" }>
```

When `direction = "both"`, response rows tag the direction. When unidirectional, no tag (it's implicit).

**Rationale.** Both directions are first-class for package extraction (transcript T4). One tool with a direction parameter beats two parallel tools.

### D12. Runtime: Rust + rmcp + stdio (v0)

```
Implementation: a Rust binary `kenn-mcp` (or subcommand `kenn mcp`)
                using the official `rmcp` SDK from
                github.com/modelcontextprotocol/rust-sdk

Transport:      stdio (v0)
                Future: MCPB packaging — bundles the binary + .kenn/
                config, install via Claude Desktop/Code without Rust toolchain
                on user's box

Workspace:      single workspace per server instance. The MCP server is
                spawned with the workspace root as a CLI arg or cwd; reads
                .kenn/live/ snapshot only.

Reindex:        out-of-band, via `kenn index` CLI. The MCP server
                detects via filesystem watch + atomic-symlink-flip
                (per indexed-store-and-lifecycle/D6) and starts serving
                from the new snapshot on next request boundary. No
                reload-required tool.
```

**Rationale.**
- Rust shares crates with the indexer/store; no marshaling between languages.
- `rmcp` is the first-party SDK; stdio is the simplest transport for local-only use.
- MCPB packaging is mechanical and post-MVP; we avoid carrying packaging design before the binary exists.

### D13. Multi-workspace: one MCP server per workspace, by design

A single MCP server instance reads from one workspace's `.kenn/` directory. Agents that work across workspaces register multiple MCP servers (one per workspace).

**Rationale.**
- Cross-workspace queries are a federation problem (transitive imports across repos, cross-repo refactors). Different schema concerns; not in v1.
- Sharing the indexer/store crate per-workspace keeps the implementation tight.

### D14. Error model: JSON-RPC errors for unrecoverable, envelope for recoverable

| Condition | Mechanism | Code |
|---|---|---|
| stale cursor (mid-pagination reindex) | JSON-RPC error | `STALE_CURSOR` |
| index unavailable (no `.kenn/live`) | JSON-RPC error | `INDEX_UNAVAILABLE` |
| invalid id format / unparseable cursor | JSON-RPC error | `INVALID_INPUT` |
| `get_symbol` for nonexistent id | success, `{ found: false, not_found: {...} }` | — |
| `list_*` no matches | success, `{ items: [], total: 0, next: null }` | — |
| db query failure / corrupt snapshot | JSON-RPC error | `INTERNAL_ERROR` |

**Rationale.** Agent-recoverable conditions return data so the agent doesn't need exception handling. Unrecoverable conditions (stale cursor, missing index) need user/agent action and surface as errors.

## Risks / Trade-offs

- **[Risk] Tool descriptions burn context every turn.** With 15 tools at ~150 tokens each, ~2.5 KB context per turn just for tool definitions. Acceptable but tight; descriptions must be terse. Mitigated by writing them short and in `tasks.md` carrying explicit length guidance.
- **[Risk] STALE_CURSOR in long agent sessions.** A multi-minute agent loop iterating with pagination can hit a reindex. Acceptable: agent restarts (the analysis would have been wrong anyway). Documented in tool descriptions.
- **[Risk] include_tests defaults differ per tool.** Slight learning curve. Mitigated: every tool's description states the default explicitly.
- **[Risk] count_only doesn't compose with `next` cursor.** When `count_only=true`, response always has `next=null`; mixing them is incoherent. Spec'd as: `count_only=true` ignores any cursor and returns `{ items: [], total, next: null }`.
- **[Trade-off] No site-level call queries.** Agents reading source instead means more file I/O on the agent side. Acceptable; that's outside our DB anyway.
- **[Trade-off] No suggestion engine for `not_found`.** Agents get parent_id+parent_kind; they retry with `find_symbols`. Suggestion engines (fuzzy match within parent scope, recent rename detection) are a v2 enhancement.
- **[Trade-off] One workspace per server.** Cross-workspace use cases require host configuration of multiple servers. Acceptable for v1; federation is its own design problem.

## Migration Plan

Greenfield. No migration. Implementation depends on `source-data-model`, `indexed-store-and-lifecycle`, and `scip-indexing-pipeline` being implemented first; this proposal is the read consumer on top.

## Open Questions

- **rmcp SDK feature parity.** The Rust MCP SDK lags TS on bleeding-edge features. v1 uses only baseline tool calls and stdio — fully supported. Confirm no missing dependency at implementation time.
- **Tool description length budget.** Target ~150 tokens per tool description (~2.5 KB total). Verify against actual Claude tool-list usage during dev iteration.
- **BM25 score stability across pagination.** SurrealDB's BM25 is deterministic for a given snapshot; cursor includes `last_bm25_score` so resumption is unambiguous. Verify there is no implicit randomness in tie-breaking; if there is, add `last_short_id` as the deterministic tiebreaker (already in the cursor format).
- **MCPB packaging strategy.** Deferred to post-MVP. Rust binary cross-compilation matrix (macOS arm64/x86_64, Linux x86_64/arm64, Windows x86_64) needs to be sorted before MCPB ships.
- **Test discovery as a tool.** `is_test` filter handles "include/exclude tests" but not "find tests for symbol X". Currently no dedicated tool; agent uses `list_usages(id, filters: { include_tests: true, kind: [...] })` and filters client-side. Adequate for v1; revisit if friction surfaces.
