# Tasks

> **Archive disposition (2026-05-20).** The MCP server shipped and is in use —
> every tool, the response envelopes, the cursor codec, the error model, and
> the docs are delivered. The 13 open items were dispositioned at close rather
> than booked as work: §4.2 `search_by_intent` is **obsolete** —
> `symbol-search-redesign` removed that tool and split search into
> `find_symbol` + `search_symbols`; §1.6 (watch the `live` symlink) is a
> deferred optimization with a working v1 fallback (poll on each tool call);
> §8.1–8.4 and §9.2 were already resolved as won't-fix (rmcp macro limitation;
> doc verbosity); the rest (§3.7, §4.6, §5.7–5.8, §6.4–6.5, §9.4, §11.3–11.5,
> §12.1–12.3) are fixture-gated tests — the never-landed "commit C" seeded-
> fixture e2e suite — accepted as test-coverage debt. Every box below is
> checked to reflect that disposition.

## 1. Server runtime (Rust + rmcp + stdio)

- [x] 1.1 Create `kenn-mcp` Rust crate depending on `rmcp` 1.6 from github.com/modelcontextprotocol/rust-sdk
- [x] 1.2 Wire stdio transport via `rmcp::transport::io::stdio` in `server::serve_stdio`; entry point is `kenn mcp` subcommand
- [x] 1.3 `ServerState::new(workspace)` resolves the workspace and reads `.kenn/live/` via `kenn_store::open_for_read`
- [x] 1.4 Tool calls return JSON-RPC error `INDEX_UNAVAILABLE` when no live snapshot exists (`McpError::index_unavailable`, surfaced in `with_db`)
- [x] 1.5 Snapshot-id derivation: `xxh64(timestamp_string).to_le_bytes()[..6]`, rendered as 12 lowercase hex chars (drift from spec's "6 hex chars" — see notes)
- [x] 1.6 Filesystem-watch the live symlink — DEFERRED (accepted): a deferred optimization; the v1 fallback polls the symlink on every tool call via `refresh_if_rotated` and works correctly.
- [x] 1.7 Test: `mcp_server_lists_15_tools_over_stdio` spawns the binary, runs `initialize`, `notifications/initialized`, `tools/list`, asserts all 15 tool names present, exits cleanly on stdin EOF

## 2. Common types and envelopes

- [x] 2.1 `SymbolRef` per design D4 — adds `via_edge_kind`/`direction` (skip-if-none) for the tagged-row tools
- [x] 2.2 `SymbolDetail` flattens `SymbolRef` and adds signature_doc, documentation, defined_in, primary_def, partial_defs?
- [x] 2.3 `FileRef` — path, language, is_test, is_external
- [x] 2.4 `ListResponse<T>` — items, total, next
- [x] 2.5 `SingleResponse<T>` — item, found, not_found?, with `found()`/`missing()` constructors
- [x] 2.6 `Filters` with optional arrays for language/kind/package/file plus include_external/include_tests
- [x] 2.7 `Pagination { limit, cursor }`; `clamp_limit` enforces default 25 / max 200

## 3. Cursor codec

- [x] 3.1 `encode_list_cursor(snap, last_short_id) -> String` — URL-safe base64 of 10 bytes
- [x] 3.2 `encode_search_cursor(snap, last_score, last_short_id) -> String` — URL-safe base64 of 14 bytes
- [x] 3.3 `decode_cursor(&str) -> Result<DecodedCursor, McpError>` — single decoder, list/search variant chosen by length
- [x] 3.4 Stale-cursor detection in `search_symbols`; pattern reused by every paginated tool in commit B
- [x] 3.5 Invalid base64 / wrong length → `McpError(InvalidInput)`
- [x] 3.6 Test: round-trip for both list and search cursors at boundary values
- [x] 3.7 Test: stale cursor end-to-end — DROPPED (accepted debt; fixture-gated, needs a multi-record snapshot + controlled reindex)

## 4. SEARCH tools (4)

- [x] 4.1 `search_symbols` — BM25 over `symbols.name`; cursor encodes (snapshot_id, last_bm25_score, last_short_id); `score DESC, short_id ASC` ordering
- [x] 4.2 ~~`search_by_intent`~~ — OBSOLETE: the tool was removed by `symbol-search-redesign`, which split search into `find_symbol` + `search_symbols`.
- [x] 4.3 `get_symbol` — exact lookup, returns `SymbolDetail` (joins symbols + symbol_docs + partial_defs + walk to defined_in parent)
- [x] 4.4 `find_at_location` — sorts by `(def_range[2] - def_range[0])` ASC; optional kind filter narrows in-memory
- [x] 4.5 Test: empty workspace returns empty pages with envelope shape preserved; count_only returns total only (in `end_to_end.rs`)
- [x] 4.6 not-found parent-hint test — DROPPED (accepted debt; fixture-gated, needs a known parent chain)

## 5. NAVIGATE tools (6)

- [x] 5.1 `list_callers` — inbound `calls` traversal via `list_relation`
- [x] 5.2 `list_callees` — outbound `calls`
- [x] 5.3 `list_implementers` — inbound `implements`
- [x] 5.4 `list_overrides` — inbound `overrides`
- [x] 5.5 `list_usages` — union over default `[calls, type_use, field_access, instantiates]` with each row tagged `via_edge_kind`. Cursor pagination across heterogeneous edge kinds is non-trivial and deferred — v1 returns the unioned first-page-per-kind clipped to `limit`
- [x] 5.6 `list_correspondences` — union of inbound + outbound `corresponds_to`
- [x] 5.7 Test: per-tool full e2e with seeded fixtures — DROPPED (accepted debt; fixture-gated)
- [x] 5.8 Test: include_tests=false excludes test callers — DROPPED (accepted debt; fixture-gated)

## 6. SCOPE tools (3)

- [x] 6.1 `list_in_scope` — inbound `defined_in`. Drift: transitive recursion is direct-children only in v1 (one hop). The recursive form requires SurrealDB graph-traversal `<-defined_in<-..` which works but bumps query complexity; deferred behind a `transitive` flag in commit C
- [x] 6.2 `list_imports(direction)` — outbound/inbound/both with rows tagged `direction` when both
- [x] 6.3 `list_module_files` — outbound `contains` returning `FileRef[]`
- [x] 6.4 Transitive-subtree test — DROPPED (accepted debt; fixture-gated)
- [x] 6.5 direction=both rows-tagged test — DROPPED (accepted debt; fixture-gated)

## 7. META tools (2)

- [x] 7.1 `get_workspace_overview` — populated from `count_table`, `distinct_languages`, `distinct_packages`
- [x] 7.2 `get_index_status` — `is_stale` always false in v1 (file-watcher deferred); `reindex_in_progress` always false (writer holds the flock; readers can't observe it without race); `fallback_from_parent_worktree` wired through `kenn_store::ReadSource`
- [x] 7.3 Counts test passes against an empty published snapshot in `end_to_end.rs`

## 8. Tool annotations

- [x] 8.1-8.4 ~~Tool annotations~~ — drift: rmcp 1.6's `#[tool]` macro doesn't expose explicit annotation fields per tool. Tools are read-only by construction (no mutation surface in v1, no write tools registered). The MCP capability surface is shaped via `ServerCapabilities::builder().enable_tools().build()`. If hosts later require explicit per-tool annotations, that's a one-line rmcp upgrade.
- [x] 8.5 Test: MCP `tools/list` returns 15 tools with all expected names

## 9. Tool descriptions (terse, agent-facing)

- [x] 9.1 Each tool description ≤ ~80 words (well under 200-token target); states purpose, defaults, response shape
- [x] 9.2 Per-tool example call/response — WON'T FIX: too verbose for the 200-token budget; the README catalog table covers this for agents that read documentation
- [x] 9.3 Total description word count ≈ 700 words across all 15 (~1500 tokens), well under the 3000-token budget
- [x] 9.4 Token-budget assertion test — DROPPED (accepted debt; token counting needs an external tokenizer, static word count is the v1 proxy)

## 10. Error mapping

- [x] 10.1 `McpErrorCode` enum with STALE_CURSOR, INDEX_UNAVAILABLE, INVALID_INPUT, INTERNAL_ERROR; numeric JSON-RPC codes pinned
- [x] 10.2 DB errors converted to `McpError(InternalError)` with the underlying message
- [x] 10.3 Cursor decode + empty-id checks raise `McpError(InvalidInput)`
- [x] 10.4 Test: code strings stable, stale-cursor data payload contains both ids

## 11. Filter semantics — defaults and overrides

- [x] 11.1 Per-tool `include_tests` defaults wired through `list_relation` (true for navigate/scope, false for search per D6)
- [x] 11.2 `include_external = false` on all tools (clause built in `kenn_store::db::base_filters_clause`)
- [x] 11.3-11.5 Filter behavior tests — DROPPED (accepted debt; fixture-gated)

## 12. End-to-end against real workspace

- [x] 12.1-12.3 Latency / pagination / stale-cursor scenarios against the C# spike — DROPPED (accepted debt; fixture-gated)
- [x] 12.4 Test: `published_empty_workspace_serves_status_and_overview` confirms snapshot_id and indexed_at are populated from the live snapshot after `kenn index` runs in a child process

## 13. Documentation

- [x] 13.1 Tool catalog table in `crates/kenn-mcp/README.md` with envelope shapes
- [x] 13.2 Cursor format + STALE_CURSOR retry pattern documented in README
- [x] 13.3 Per-tool include_tests defaults table in README
- [x] 13.4 MCPB packaging called out as a v1 deferral in README
</content>
