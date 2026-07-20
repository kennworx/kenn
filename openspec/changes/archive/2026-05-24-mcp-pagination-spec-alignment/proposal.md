## Why

The MCP spec (2025-11-25, `server/utilities/pagination`) defines a precise
contract for cursor-based pagination: opaque tokens, server-decided page
size, `-32602` on invalid cursor, `nextCursor` present only when more
results exist. Kenn-mcp already implements cursor pagination on tool
results (`cursor.rs` → `search_symbols`, `list_callers`, etc.) and on
`tools/list` (via rmcp defaults), but the rules aren't pinned by spec.

Two reasons to pin them now:

1. **MCP-host portability.** Anything we ship for Claude Code today
   should also work cleanly with hosts that strictly follow the spec
   (Cursor, Zed via ACP, custom HTTP clients via rmcp). Documenting the
   contract makes a cross-host regression test possible.
2. **Stop the foot-guns.** The current code uses `STALE_CURSOR` for one
   failure mode and `INVALID_CURSOR` for another. The spec mandates
   `-32602 Invalid params` for both. Aligning prevents subtle host-side
   error-handling drift.

This is a contract-tightening change, not a behavioral one. Most
requirements describe what kenn-mcp already does; one or two adjust
error codes and the `nextCursor`-emission rule.

Reference: <https://modelcontextprotocol.io/specification/2025-11-25/server/utilities/pagination>

## What Changes

### Tool-result pagination (kenn-specific, custom contract)

The `cursor.rs` cursor codec (snapshot_id + position) stays. What gets
spec-aligned:

- **Opacity**: cursors continue to be base64-encoded binary; reaffirm
  that the agent MUST NOT parse or persist them across sessions.
- **Page size**: page size is server-decided per the spec; the
  `limit` parameter on paginated tools (`search_symbols`,
  `list_callers`, `list_callees`, etc.) is a server-controlled
  *ceiling*, capped to 200 server-side. Document this as the spec's
  "page size is server-decided" rule applied to our extension.
- **`nextCursor`-emission rule**: emit a continuation cursor only when
  the underlying result stream has more rows. Empty / final pages MUST
  omit it. Today some paths emit a cursor on every page including the
  last — fix this.
- **Invalid cursor**: return JSON-RPC `-32602 Invalid params` on any
  malformed cursor (bad base64, truncated, wrong-shape). Today we
  return our own `INVALID_CURSOR` error code; switch to the spec
  contract.
- **Stale cursor (snapshot mismatch)**: keep the `STALE_CURSOR`
  semantic — the cursor was valid but the snapshot rotated — but
  return it as `-32602 Invalid params` per the spec, with the
  human-readable reason in `data.kenn_subcode: "STALE_CURSOR"` so the
  agent can distinguish "restart pagination" from "fix your cursor."

### MCP-spec-mandated pagination (`tools/list`)

The spec lists `tools/list`, `resources/list`, `resources/templates/
list`, and `prompts/list` as the paginated operations. Kenn-mcp serves
`tools/list` (28 tools — 24 query + the lifecycle `reindex`,
`watch_start`, `watch_stop` + the `debug_env` diagnostic, per
`server.rs:1`); the other three are not implemented (no resources,
no prompts in this server).

- `tools/list` SHALL conform to the MCP pagination contract: opaque
  cursor, server-decided page size, `nextCursor` only when more, no
  client-assumed size. With ~25 tools we will likely never emit
  `nextCursor` in practice, but the contract holds for future growth.
- The other paginated operations are out of scope (no resources / no
  prompts exposed by kenn-mcp today; their pagination becomes
  in-scope automatically if those surfaces are ever added).

### Out of scope

- Adding pagination to non-paginated tools (`get_symbol`, `get_source`,
  `get_index_status`). These return single records or fixed-shape
  status, not lists.
- Changing the cursor *format*. The existing 14/20-byte shape works.

## Capabilities

### Modified Capabilities

- `mcp-server`: gains explicit pagination-contract requirements. No
  behavior change for callers that use cursors correctly; small error-
  code adjustment for callers that abuse them.
