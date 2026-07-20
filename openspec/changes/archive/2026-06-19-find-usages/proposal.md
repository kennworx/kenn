## Why

"Where is X used?" is the most common code-intel question, and kenn currently
makes it cost **two MCP roundtrips**: `find_symbol(name)` → take the `id` →
`list_usages(id, …)`. Each roundtrip is a full LLM turn — latency and tokens —
so the hottest path pays the highest tax. The two-step exists only because the
tools are shaped around graph *primitives* (resolve, then traverse) rather than
the agent's *intent* (find the references). The resolve-then-traverse is a
server-side join; the agent shouldn't have to be the glue.

## What Changes

- Add a single fused tool **`find_usages(query, …)`** that resolves a name / path
  / `pub_id` to its node(s) **and** returns the incoming references in **one
  call**. The existing primitives (`find_symbol`, `list_usages`, `list_callers`)
  stay for power use.
- **Ambiguity is surfaced in the response, not via a second roundtrip.** When
  `query` resolves to several nodes, results come back as **one flat reference
  list, each row tagged with its resolved target** (uniform shape, unique or
  ambiguous) — the agent gets everything in one call and groups by the tag if it
  wants. Optional narrowing filters (`kind`, `path`, `package`, `language`) let a
  caller that already knows which target it wants pin it down, still in one call.
- The query is **string-first with optional structured narrowing** — a required
  `query` string plus optional scalar filters — **not** a `string | object`
  union (LLMs handle unions poorly) and **not** split into per-type tools
  (`find_symbol_usages` / `find_file_usages`), which would only disambiguate the
  cheap axis (which table) while leaving the real one (which of N matches)
  unsolved, and would multiply the tool surface for every fused intent.

## Capabilities

### New Capabilities

- `mcp-find-usages`: the fused reverse-lookup tool — query resolution dispatched
  by form (name → name index, path → file lookup, asset → attachment stub,
  `pub_id` → direct) joined with incoming-edge traversal in one call, a flat
  reference list tagged by resolved target, optional narrowing filters, and an
  edge-kind selector (defaulting to the reference-style edges incl. `imports`).
  It is a **search-style** tool — an empty result is valid (used-nowhere), not an
  error. Pagination applies only when the query resolves to a **single** target;
  multiple targets return `next: null` with truncation reported, and the tool
  description tells the agent to narrow (filter or `pub_id`) to paginate.

## Impact

- **Roundtrips:** the hot "where used" path drops from 2 calls to 1 — half the
  latency and tokens for the most frequent query.
- **Surface:** one new MCP tool in `crates/kenn-mcp/src/tools/`; it composes the
  existing resolver + edge-traversal, no new store capability. Added to the
  `mcp-server` paginated-tool, empty-snapshot, and **search-tool-exemption** lists
  (not the unresolved-entity-error list).
- **Generality:** works for symbols, files, and attachment stubs alike — so it
  also makes the `index-html` asset reverse-lookup (`<img src>` → who references
  it) a single call. (That asset case relies on `index-html` keying attachment
  stubs by canonical workspace-relative path, so a path query resolves
  deterministically — specified in `index-html` `html-index`.)
- **No breaking change:** the primitives are untouched; this is additive.
