## Context

The reverse-lookup intent ("who references X") is served today by chaining
`find_symbol` → `list_usages`. Two MCP calls = two LLM roundtrips. The cost is
structural: the tools mirror the graph's primitives, so the agent assembles them.
This change adds one intent-shaped tool that does the join server-side.

## Goals / Non-Goals

**Goals:**

- One call from a human-meaningful handle (name / path / `pub_id`) to its
  incoming references.
- Resolve *match* ambiguity in the response, never with a second roundtrip.
- Stay consistent with kenn's existing filter and pagination vocabulary.

**Non-Goals:**

- Removing or changing the primitives (`find_symbol`, `list_usages`,
  `list_callers`). They remain for stepwise/power use.
- A general graph-query language. This is one fused intent, not a DSL.
- Transitive/multi-hop usage (that is a separate `blast`-style intent).

## Decisions

### D1 — Two ambiguities; the per-type split only solves the cheap one

```
   A — which table?   "logo.png" → a file, or a symbol of that name?
   B — which match?   "Order" → 5 symbols; "config.rs" → 3 files in 3 dirs
```

Splitting into `find_symbol_usages` / `find_file_usages` resolves **A** by making
the caller pick the tool — but **A is inferable** from the query shape (a `/` or a
file extension ⇒ file) or moot (search both). It does nothing for **B**, the
ambiguity that actually bites. So the split ships two tools and still needs
grouped results for B. Rejected.

### D2 — One tool: string-first, optional structured narrowing, grouped response

```
find_usages({
  query,                       // required — name | path | pub_id
  kind?, path?, package?, language?,   // optional narrowing (the existing Filters vocab)
  edge_kinds?,                 // default: the reference-style edges (see D3)
  include_external?, page_size?, cursor?
}) → references GROUPED by resolved target
```

- **Uniform response, one cheap call:** the response is always a **flat list of
  references, each row tagged with the resolved target it points at** — not a
  nested group structure. A unique resolution tags every row with the same
  target; an ambiguous one (B) interleaves rows for several targets. The agent
  groups by the tag client-side if it wants. This keeps the shape uniform
  (no flat-vs-grouped branching) and keeps cursor pagination a plain flat walk.
- **Resolution dispatches on query form:** a `pub_id` is used directly; a
  workspace-relative **path** resolves via the file lookup (`fetch_file_short_id`)
  or, for a non-indexed asset, its attachment stub; a plain **name** goes through
  the `find_symbol` name index. `find_symbol` alone cannot resolve a path — name
  vs path vs id are distinct resolution routes, not one.
- **Precise case, also one call:** the caller that already knows the target
  passes a `pub_id` as `query` (skips resolution) or adds `kind:`/`path:` to pin it.
- **String + optional filters, not a `string | object` union.** A required string
  with optional scalar filters is LLM-friendly; `oneOf`/`anyOf` tool schemas are
  fumbled by models. The "structure" the caller can supply is *narrowing on top
  of* the string, never a replacement for it.
- **Reuses the existing `Filters` enum** (`kind`/`package`/`language`/`path`) that
  `find_symbol`/`search_symbols` already take — no new dialect.

### D3 — Default edge set = reference-style; overridable

`list_usages` defaults to `[calls, type_use, field_access, instantiates]`.
`find_usages` widens the default to the **reference-style** edges so "where used"
covers imports, links, and class usage too: `[calls, type_use, field_access,
instantiates, imports, links_to, links_to_file, embeds, uses_css_class]`.
`imports` is load-bearing for **file/stylesheet** targets — "who references
`app.css`" is its `<link>` importers, an `imports` edge; omitting it would make
`find_usages` on a file silently miss them. `edge_kinds` overrides the default.
This is also what makes the `index-html` asset case work: `find_usages("assets/logo.png")`
returns the `links_to` references to the attachment stub.

### D4 — find_usages is a search-style tool: empty is valid, NOT an error

`find_usages` is query-shaped, so it takes the `mcp-server` **search-tool
exemption** — the same one `find_symbol`/`search_symbols` have: *an empty result
is the correct answer to a query that matched nothing.* This is a correctness
requirement, not a style choice: the no-stub-means-unused property the
`index-html` design relies on (an asset referenced nowhere has no node) means
`find_usages("unused.png")` must answer **"used nowhere" (empty)**, never an
unresolved-entity error. Lumping it with the error-on-unresolved tools would break
exactly the asset reverse-lookup this enables.

It still returns the *empty-snapshot → error* (no index at all is different from a
real query with zero hits). Registration: add it to the `mcp-server` empty-snapshot
and **search-tool exemption** lists — **not** the unresolved-entity-error list.

**Pagination is single-target only.** kenn's multi-relation aggregators
(`list_usages`, `list_correspondences`) return `next: null` — only single-relation
tools cursor-paginate (`list_relation`). `find_usages` follows suit: when the query
resolves to **one** target it paginates that target's references with a cursor
(an edge-kind-ordinal + last-short-id over the fixed target); when it resolves to
**several**, it caps + reports truncation + `next: null`, and the **tool
description tells the agent to narrow** (filter or `pub_id`) to a single target to
paginate. This avoids inventing cross-target pagination (rejected option B) while
still giving a paginating path once the target is pinned.

## Risks / Trade-offs

- **[Ambiguity explosion]** a very common name (`get`) could resolve to hundreds
  of nodes → the grouped response is huge. Mitigation: cap the number of resolved
  target groups (e.g. top-N by relevance, like `search_symbols`' top-K), report
  truncation, and tell the caller to narrow with `kind:`/`package:`.
- **[Two ways to do one thing]** the tool overlaps the primitives. Mitigation:
  the primitives are for stepwise traversal; `find_usages` is the one-shot intent.
  The skill/docs point hot-path callers at `find_usages` and keep the primitives
  for when the agent is already holding an `id`.

## Open Questions

- **Resolved-target cap size** — what N for the top-N target cap on a very common
  name, and is relevance ranking (like `search_symbols` top-K) the right order?
  Decide with real query shapes at implementation.
- **Should `find_callers`/`find_implements` get the same fusion?** Same principle
  applies; defer until `find_usages` proves the shape, to avoid surface bloat.
