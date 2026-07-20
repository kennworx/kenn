## ADDED Requirements

### Requirement: Paginated tool results MUST use opaque, server-controlled cursors

Every kenn-mcp tool that returns a list SHALL paginate its results using opaque cursor tokens that follow the MCP pagination contract. The paginated tools include `search_symbols`, `list_callers`, `list_callees`, `list_usages`, `list_in_scope`, `list_implementers`, `list_overrides`, `list_correspondences`, `list_imports`, `list_module_files`, `find_symbol`, and `find_similar`.

The cursor SHALL be opaque to callers. Callers MUST NOT parse, modify,
or persist cursors across sessions. The server SHALL decide page size;
the `limit` parameter on paginated tools is a server-controlled
ceiling capped at 200 server-side and MUST NOT be interpreted by
clients as a guaranteed page size.

The server SHALL emit a continuation cursor in the response if and
only if the underlying result stream has more rows after the returned
page. A final page MUST omit the cursor entirely. Clients receiving a
response without a continuation cursor MUST treat the stream as
exhausted.

#### Scenario: cursor opacity

- **WHEN** an agent calls a paginated tool and receives a continuation cursor
- **THEN** the agent treats the cursor as an opaque string with no documented format
- **AND** the only valid action is to pass it verbatim back to the same tool
- **AND** the agent does not parse, decode, modify, or persist the cursor across sessions

#### Scenario: nextCursor only when more

- **GIVEN** a paginated tool whose result set has exactly N rows
- **WHEN** the tool is walked with page size that exactly consumes all N rows in the final page
- **THEN** the final page response MUST omit any continuation cursor
- **AND** passing back a previously-issued cursor from this stream returns an empty page with no continuation cursor

#### Scenario: server-decided page size

- **GIVEN** a paginated tool with `limit` not specified by the caller
- **WHEN** the tool runs
- **THEN** the server returns a page of size determined by server policy (default 25, hard cap 200)
- **AND** the caller cannot assume any specific size before reading the response

### Requirement: Invalid and stale cursors MUST return `-32602`

The server SHALL return JSON-RPC `-32602 Invalid params` for any cursor that cannot be decoded (bad base64, wrong length, structural mismatch), per the MCP pagination spec.

A cursor that decodes correctly but references a snapshot that no
longer matches the live index (snapshot rotated between calls) SHALL
also produce `-32602 Invalid params`, with a kenn-specific subcode in
the error's `data` payload so the agent can distinguish "your cursor
was malformed" from "you need to restart pagination because the index
rotated."

The error's `data` payload SHALL include a `kenn_subcode` field with
one of:

- `"INVALID_CURSOR"` — the cursor could not be decoded.
- `"STALE_CURSOR"` — the cursor decoded but the snapshot no longer matches.

#### Scenario: malformed cursor

- **WHEN** an agent calls a paginated tool with a cursor whose length has been changed (truncated, padded, or whose base64 decodes to a wrong byte count for either the 10-byte list shape or 14-byte search shape)
- **THEN** the server returns `-32602 Invalid params`
- **AND** the error's `data.kenn_subcode` is `"INVALID_CURSOR"`

Note: a cursor whose content has been mutated *without* changing its
length usually decodes successfully but points to a non-existent or
wrong-snapshot position; that path returns either `STALE_CURSOR`
(when the snapshot prefix no longer matches) or a valid-shaped empty
page (when the position is past the data). The deterministic way to
trigger `INVALID_CURSOR` is a length-mutation.

#### Scenario: stale cursor across a snapshot rotation

- **GIVEN** an agent has a valid continuation cursor from a previous page
- **WHEN** the index rotates to a new snapshot between calls
- **AND** the agent passes the old cursor back
- **THEN** the server returns `-32602 Invalid params`
- **AND** the error's `data.kenn_subcode` is `"STALE_CURSOR"`
- **AND** the agent's correct action is to restart pagination from the beginning, not to "fix" the cursor

### Requirement: `tools/list` MUST conform to MCP pagination

The kenn-mcp `tools/list` response SHALL conform to the MCP
pagination contract: opaque cursor, server-decided page size,
`nextCursor` present only when more tools follow.

Kenn-mcp's tool count is small enough that `tools/list` typically
returns a single page; the contract holds for future growth and for
host conformance testing.

#### Scenario: tools/list single page

- **WHEN** a client calls `tools/list` and the full tool set fits in one server-decided page
- **THEN** the response is `{ tools: [...] }` with no `nextCursor` field
- **AND** the client treats this as the complete tool list

#### Scenario: tools/list cursor round-trip

- **GIVEN** a future kenn-mcp build large enough that `tools/list` paginates
- **WHEN** the client walks pages until `nextCursor` is absent
- **THEN** the union of returned tools equals the full tool set
- **AND** no tool appears in more than one page
