## ADDED Requirements

### Requirement: find_usages resolves and traverses in one call

The MCP server SHALL expose a `find_usages` tool that, in a single call, resolves
a `query` to its node(s) and returns the **incoming** references to them, without
requiring the caller to first `find_symbol` then `list_usages` — the
resolution-plus-traversal join is performed server-side. Resolution SHALL dispatch
on the query form: a **`pub_id`** is used directly (no resolution); a
**workspace-relative path** resolves to that file node (the file lookup, not the
name index) or, when the path names a non-indexed asset, to its attachment stub; a
plain **name** resolves through the `find_symbol` name index. Each returned
reference SHALL carry its source node, the resolved target it points at, and the
edge kind by which it references that target.

#### Scenario: name to references in one call

- **WHEN** the agent calls `find_usages("OrderHandler")`
- **THEN** the response lists the nodes that reference `OrderHandler`, each tagged
  with its edge kind, without a prior `find_symbol` call

#### Scenario: a path resolves to a file node, not the name index

- **WHEN** the agent calls `find_usages("src/orders/api.ts")`
- **THEN** the query resolves via the file lookup to that file node and returns its
  incoming references (e.g. `imports`, `links_to_file`)

#### Scenario: an asset path to its references

- **WHEN** the agent calls `find_usages("assets/logo.png")` and the asset is
  referenced by `<img src>` / `![[…]]`
- **THEN** the path resolves to the attachment stub and the response lists those
  referencing nodes via their `links_to` edges

### Requirement: match ambiguity is surfaced in the response, not a second roundtrip

When `query` resolves to more than one node, `find_usages` SHALL return all the
references in **one response as a flat list, each reference tagged with the
resolved target it belongs to** — not a nested group structure — so the caller
gets the full answer in one call and can group by the target field client-side
(this keeps the response shape uniform whether resolution is unique or ambiguous,
and keeps cursor pagination a simple flat walk). The tool SHALL accept optional
narrowing filters — `kind`, `path`, `package`, `language` (the existing search
`Filters` vocabulary) — that restrict resolution to a single intended target,
still in one call. The number of distinct resolved targets MAY be capped (top-N by
relevance); when capped, the response SHALL report the truncation.

#### Scenario: an ambiguous name returns one flat tagged list

- **WHEN** `find_usages("Order")` resolves to several symbols
- **THEN** the response is a single flat list of references, each tagged with which
  resolved `Order` symbol it points at

#### Scenario: a narrowing filter pins a single target

- **WHEN** `find_usages("Order", kind: "class")` resolves to exactly one class
- **THEN** the response lists that class's references (every row tagged with the
  same target)

#### Scenario: many resolved targets are capped with truncation reported

- **WHEN** `find_usages("get")` resolves to more targets than the cap
- **THEN** the response covers the top-N targets and reports that the target set
  was truncated

### Requirement: find_usages defaults to reference-style edges and is overridable

`find_usages` SHALL default its edge selection to the reference-style edges —
`calls`, `type_use`, `field_access`, `instantiates`, `imports`, `links_to`,
`links_to_file`, `embeds`, `uses_css_class` — so "where used" spans calls,
type/field references, module imports, document links, and class usage (the
`imports` edge is required so a file/stylesheet target surfaces its `<link>`/
`<script>`/module importers). An explicit `edge_kinds` argument SHALL override the
default.

#### Scenario: default spans imports, links, and class usage

- **WHEN** `find_usages` is called with no `edge_kinds`
- **THEN** the default edge set includes `imports`, the link edges, and
  `uses_css_class`, not only call/type edges

#### Scenario: explicit edge_kinds narrows the traversal

- **WHEN** `find_usages("Order", edge_kinds: ["calls"])` is called
- **THEN** only `calls` references are returned

### Requirement: find_usages is a search-style tool — an empty result is valid

`find_usages` is query-shaped, so it SHALL follow the `mcp-server` **search-tool**
contract (the same exemption as `find_symbol`/`search_symbols`): a query that
resolves to no node, or to a real entity with zero incoming references, SHALL
return an **empty result, not an error**. This is required for correctness — an
asset or file that exists but is referenced nowhere has no stub/usages, and "used
nowhere" is the right answer, not a failure. It SHALL still return the structured
empty-snapshot error when the whole snapshot has zero symbols, and SHALL be added
to the `mcp-server` search-tool-exemption and empty-snapshot lists, but **not** the
unresolved-entity-error list.

#### Scenario: an unreferenced real asset returns empty, not an error

- **WHEN** `find_usages("assets/unused.png")` names a real asset that nothing
  references
- **THEN** an empty result is returned (used nowhere), not an unresolved-entity
  error

#### Scenario: a meaningless query returns empty

- **WHEN** `find_usages("DefinitelyNotAName")` matches no node
- **THEN** an empty result is returned (search-style), consistent with `find_symbol`

### Requirement: pagination is available only for a single resolved target

`find_usages` SHALL paginate **only when the query resolves to exactly one
target** — pagination across a union of several targets is not coherent, and
kenn's existing multi-relation aggregators (`list_usages`,
`list_correspondences`) do not paginate. For a single target it returns an opaque
server-controlled cursor that round-trips, walking that target's incoming
references. When the query resolves to **more than one** target, the response
SHALL set `next` to null,
return the capped flat tagged list, and report truncation — the caller narrows to
a single target (via a `kind`/`path`/`package` filter or a `pub_id`) to obtain a
paginating stream. **The tool's own description SHALL state this narrow-to-paginate
behavior explicitly**, so the agent knows that an ambiguous query is not
paginated and how to get a paginating one.

#### Scenario: a single resolved target paginates

- **WHEN** `find_usages("cs:Models.Order")` (a `pub_id`, one target) has more
  references than one page
- **THEN** a `next` opaque cursor is returned and accepted verbatim on the
  follow-up call

#### Scenario: multiple targets are not paginated

- **WHEN** `find_usages("Order")` resolves to several symbols
- **THEN** `next` is null, the capped flat tagged list is returned with truncation
  reported, and the tool description directs the caller to narrow to one target

#### Scenario: narrowing a multi-target query enables pagination

- **WHEN** the caller re-issues `find_usages("Order", kind: "class")` and it now
  resolves to exactly one target
- **THEN** the response paginates that target's references with a `next` cursor
