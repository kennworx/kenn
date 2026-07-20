## ADDED Requirements

### Requirement: MCP server is read-only with 15 tools

The MCP server SHALL expose exactly fifteen tools, all annotated `readOnlyHint = true`, organized in four categories:

- **Search (4)**: `search_symbols`, `search_by_intent`, `get_symbol`, `find_at_location`
- **Navigate (6)**: `list_callers`, `list_callees`, `list_implementers`, `list_overrides`, `list_usages`, `list_correspondences`
- **Scope (3)**: `list_in_scope`, `list_imports`, `list_module_files`
- **Meta (2)**: `get_workspace_overview`, `get_index_status`

The server SHALL NOT expose any write or mutating tool. Reindex, init, and rollback are CLI-only per `indexed-store-and-lifecycle`.

#### Scenario: Tool list contains exactly the specified 15 tools

- **WHEN** a client calls `tools/list`
- **THEN** the response MUST contain exactly fifteen tools
- **AND** every tool MUST have `readOnlyHint: true` set
- **AND** every tool MUST have `destructiveHint: false`, `idempotentHint: true`, `openWorldHint: false`

### Requirement: Uniform list/search response envelope

All `list_*` and `search_*` tools and `find_at_location` SHALL return:

```typescript
type ListResponse<T> = {
  items:  T[]
  total:  number
  next:   string | null
}
```

The `total` field SHALL contain the count of all matching results regardless of pagination. The `next` field SHALL be a non-null opaque cursor when more results exist beyond the current page, and `null` when the result set is fully consumed. There SHALL NOT be a separate `truncated` boolean — `next != null` carries that meaning.

#### Scenario: Final page has next=null

- **WHEN** a list/search tool returns the final page of results
- **THEN** the response `next` MUST be `null`

#### Scenario: total reflects full match count

- **WHEN** a list/search tool returns a page where pagination truncates the result
- **THEN** the response `total` MUST equal the count of all matching rows for the given query
- **AND** `total >= items.length` MUST hold

### Requirement: Single-result tools use SingleResponse envelope

`get_symbol`, `get_workspace_overview`, and `get_index_status` SHALL return:

```typescript
type SingleResponse<T> = {
  item:        T | null
  found:       boolean
  not_found?:  { parent_id?: string, parent_kind?: Kind }
}
```

When the tool succeeds, `found = true` and `item` is non-null. When the tool fails to find the requested entity (relevant for `get_symbol` only), `found = false`, `item = null`, and `not_found` is populated with the closest existing parent_id and its kind when discoverable.

#### Scenario: get_symbol returns parent hint on miss

- **WHEN** a `get_symbol(id)` request fails to find the symbol
- **AND** an ancestor symbol of the requested ID does exist
- **THEN** the response MUST set `found = false`
- **AND** `not_found` MUST carry the closest existing ancestor's `parent_id` and `parent_kind`

#### Scenario: get_symbol on completely unknown id

- **WHEN** the requested ID has no recognizable parent in the index
- **THEN** the response MUST set `found = false`
- **AND** `not_found` MAY have empty fields (no parent guess)

### Requirement: count_only short-circuits item materialization

Every list/search tool SHALL accept a `count_only: boolean` parameter (default `false`). When `count_only = true`:
- The server MUST execute a count-only query and SHALL NOT materialize SymbolRef rows.
- The response MUST be `{ items: [], total: N, next: null }`.
- Any cursor passed alongside `count_only = true` SHALL be ignored.

#### Scenario: count_only returns count without items

- **WHEN** `list_callers(id, count_only: true)` is called
- **THEN** the response `items` MUST be empty
- **AND** the response `total` MUST equal the actual caller count
- **AND** the response `next` MUST be `null`

### Requirement: Cursor pagination with snapshot binding

Cursors SHALL be opaque, base64-encoded, and SHALL include a 6-byte snapshot identifier derived from the live snapshot's directory name. Cursors:
- For list_* tools: encode `(snapshot_id, last_short_id)` (10 raw bytes, ~14 base64 chars).
- For search_* tools: encode `(snapshot_id, last_bm25_score, last_short_id)` (14 raw bytes, ~20 base64 chars).

When a tool is called with a cursor whose snapshot_id does not match the current live snapshot, the server SHALL return a JSON-RPC error with code `STALE_CURSOR` and `data: { expected_snapshot_id, current_snapshot_id }`.

#### Scenario: Cursor binds to snapshot

- **WHEN** a tool is called with a cursor encoded against snapshot A
- **AND** the live snapshot has rotated to snapshot B
- **THEN** the server MUST return JSON-RPC error code `STALE_CURSOR`
- **AND** the error data MUST include both `expected_snapshot_id` and `current_snapshot_id`
- **AND** the server MUST NOT return any rows from snapshot B with snapshot-A pagination state

#### Scenario: Invalid cursor format returns INVALID_INPUT

- **WHEN** a tool is called with a malformed (non-base64 or wrong-length) cursor
- **THEN** the server MUST return JSON-RPC error code `INVALID_INPUT`

### Requirement: Common SymbolRef shape

Every list/search tool that returns symbols SHALL return rows of shape:

```typescript
type SymbolRef = {
  id:              string                 // public ID per source-data-model
  kind:            Kind
  language:        Language
  name:            string
  display_name:    string
  location:        string | null          // "./file_path#start" or "./file_path#start-end"
  package:         string                 // public ID of containing package; "" if none
  module:          string                 // public ID of containing module; "" if none
  args_arity:      number                 // 0 when not callable
  generic_arity:   number                 // 0 when not generic
  is_external:     boolean
  is_test:         boolean
  is_partial:      boolean
}
```

`get_symbol` returns the richer `SymbolDetail` extending `SymbolRef` with `signature_doc`, `documentation`, `defined_in`, `primary_def`, and `partial_defs?`.

`list_usages` rows SHALL additionally carry `via_edge_kind: EdgeKind` to identify which relation matched.

`list_imports` rows SHALL additionally carry `direction: "outbound" | "inbound"` when the call's direction parameter is `"both"`.

#### Scenario: Every SymbolRef carries id, kind, language, location

- **WHEN** any list/search tool returns SymbolRef rows
- **THEN** each row MUST have `id`, `kind`, `language`, and `location` populated
- **AND** `location` MAY be `null` for symbols without source locations (synthetic, external)

### Requirement: Filters use array values

The shared `Filters` object SHALL accept array values for multi-value filters and singular booleans for toggles:

```typescript
type Filters = {
  language?:           Language[]
  kind?:               Kind[]
  package?:            string[]
  file?:               string[]            // glob patterns
  include_external?:   boolean
  include_tests?:      boolean
}
```

Single-value filters SHALL be passed as a one-element array; the server SHALL NOT accept singular non-array forms.

#### Scenario: Single-language filter passed as one-element array

- **WHEN** a client wants results in only one language
- **THEN** the request MUST send `filters.language = ["csharp"]`
- **AND** the server MUST return symbols only in that language

### Requirement: Default filter values per tool, documented

`include_external` SHALL default to `false` on all tools. `include_tests` SHALL default per the following table:

| Tool | include_tests default |
|---|---|
| `search_symbols` | `false` |
| `search_by_intent` | `false` |
| `list_callers` | `true` |
| `list_callees` | `true` |
| `list_implementers` | `true` |
| `list_overrides` | `true` |
| `list_usages` | `true` |
| `list_correspondences` | `true` |
| `list_in_scope` | `true` |
| `list_imports` | `true` |
| `list_module_files` | `true` |

Each tool's MCP description SHALL explicitly state its `include_tests` default.

#### Scenario: search_symbols excludes tests by default

- **WHEN** `search_symbols(query: "Order")` is called without an `include_tests` filter
- **THEN** the response MUST exclude symbols whose `is_test = true`

#### Scenario: list_usages includes tests by default

- **WHEN** `list_usages(id: "cs:Models.Order")` is called without an `include_tests` filter
- **THEN** the response MUST include test-file callers/users alongside production ones

### Requirement: find_at_location returns smallest-enclosing-first

`find_at_location({ file, line, kind? })` SHALL return all symbols whose `def_range` covers the requested line, sorted by range size ascending — the smallest enclosing symbol first, the largest last.

When `kind` is provided, only symbols of those kinds SHALL appear in the response. The default (omitted `kind`) returns all kinds.

The `file` parameter SHALL be a workspace-relative exact path, not a glob.

#### Scenario: Stack-trace lookup returns the containing method first

- **WHEN** `find_at_location({ file: "Models/Order.cs", line: 42 })` is called
- **AND** line 42 lies inside method `LoadConfig`, which is inside class `Order`, which is inside namespace `Models`
- **THEN** the response items MUST be ordered: method `LoadConfig` first, then class `Order`, then namespace `Models`

#### Scenario: Kind filter narrows the result

- **WHEN** the same call is made with `kind: ["method"]`
- **THEN** the response MUST contain only `LoadConfig`

### Requirement: list_usages returns rows tagged with via_edge_kind

`list_usages` SHALL return SymbolRef rows where each row carries `via_edge_kind: EdgeKind` identifying which relation type matched. The default `edge_kinds` parameter SHALL be `["calls", "type_use", "field_access", "instantiates"]`. The `op_filter` parameter SHALL narrow `field_access` matches by `read` or `write` and SHALL be ignored when `field_access` is not in `edge_kinds`.

#### Scenario: list_usages tags each row by edge kind

- **WHEN** `list_usages(id: "cs:Models.Order")` is called
- **THEN** each row in `items` MUST have `via_edge_kind` populated with one of the requested edge kinds
- **AND** the agent MAY group results client-side by this field

### Requirement: list_imports supports direction parameter

`list_imports({ id, direction, kind?, ... })` SHALL accept `direction: "outbound" | "inbound" | "both"`:
- `"outbound"`: modules this module imports
- `"inbound"`: modules that import this module
- `"both"`: union of inbound and outbound; rows carry `direction: "outbound" | "inbound"`

#### Scenario: direction=both returns tagged rows

- **WHEN** `list_imports({ id: "cs:Models", direction: "both" })` is called
- **THEN** each row MUST carry `direction` field set to `"outbound"` or `"inbound"`
- **AND** the response MUST include all imports in either direction

### Requirement: get_index_status reports staleness

`get_index_status` SHALL return:

```typescript
type IndexStatus = {
  snapshot_id:                     string         // 6 hex chars
  indexed_at:                      string         // ISO-8601 timestamp
  is_stale:                        boolean        // file watcher signals files changed since indexed_at
  reindex_in_progress:             boolean
  fallback_from_parent_worktree:   boolean        // per indexed-store-and-lifecycle/D7
}
```

The `is_stale` flag SHALL be true when the workspace's file-watcher has detected changes after the snapshot was published. Reindex starts SHALL update `reindex_in_progress = true` and reset `is_stale` only on successful publication of the new snapshot.

#### Scenario: get_index_status reflects post-edit staleness

- **WHEN** the workspace is indexed
- **AND** a file in the workspace is edited after indexing
- **THEN** `get_index_status().is_stale` MUST be `true`

#### Scenario: snapshot_id matches the cursor encoding

- **WHEN** any tool is called and returns a non-null `next` cursor
- **AND** that cursor is decoded
- **THEN** the decoded snapshot_id MUST equal `get_index_status().snapshot_id`

### Requirement: Single workspace per server instance

The MCP server SHALL serve exactly one workspace per process instance. The workspace root SHALL be specified at server startup (via CLI argument or working directory). Cross-workspace queries are not in scope.

#### Scenario: Server tied to one workspace

- **WHEN** the server is spawned with workspace root `/path/to/repo`
- **THEN** all tool calls MUST query `/path/to/repo/.kenn/live/`
- **AND** tools MUST NOT accept a workspace_id parameter

### Requirement: Error model for unrecoverable conditions

The server SHALL distinguish recoverable from unrecoverable conditions:

| Condition | Mechanism |
|---|---|
| `get_symbol` for nonexistent id | success, `SingleResponse` with `found = false` |
| `list_*` no matches | success, `ListResponse` with `total = 0` |
| stale cursor | JSON-RPC error `STALE_CURSOR` |
| index unavailable / `.kenn/live/` missing | JSON-RPC error `INDEX_UNAVAILABLE` |
| invalid id format / unparseable cursor / invalid filter | JSON-RPC error `INVALID_INPUT` |
| db query failure / corrupt snapshot | JSON-RPC error `INTERNAL_ERROR` (with sanitized message) |

#### Scenario: Empty list returns success, not error

- **WHEN** `list_callers(id)` is called for a symbol with no callers
- **THEN** the response MUST be `{ items: [], total: 0, next: null }`
- **AND** the call MUST NOT raise a JSON-RPC error

#### Scenario: Index unavailable returns INDEX_UNAVAILABLE

- **WHEN** the `.kenn/live/` symlink is missing
- **AND** any tool is called
- **THEN** the server MUST return JSON-RPC error code `INDEX_UNAVAILABLE`

### Requirement: Stdio transport for v0

The v0 MCP server SHALL use the stdio transport via the `rmcp` SDK. MCPB packaging is deferred to a follow-up effort. The server SHALL NOT expose remote HTTP, SSE, or other transports in v0.

#### Scenario: Server speaks MCP over stdio

- **WHEN** the server is spawned
- **THEN** it MUST read newline-delimited JSON-RPC from stdin
- **AND** write responses to stdout
- **AND** terminate cleanly on stdin EOF
