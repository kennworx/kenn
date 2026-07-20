## MODIFIED Requirements

### Requirement: Two distinct symbol-search tools partition the search space

The MCP server SHALL expose two symbol-search tools whose contracts make their
intended use cases mutually exclusive:

- `find_symbol(name, kind?, page_size?, include_tests?, include_external?)` —
  for the **identifier** case: the agent has a literal name (from a stack
  trace, task description, prior tool output) and wants exact / near-exact
  matches.
- `search_symbols(query, filters?, pagination?)` — for the **intent** case:
  the agent has a natural-language phrase or a loosely-recalled term and wants
  top-ranked relevance over symbol names AND documentation, surfaced as a
  paginated stream over a fixed top-K window.

The MCP descriptions of both tools SHALL state which case each is for and
explicitly steer the agent toward the other tool for the opposite case.
`search_symbols` SHALL NOT attempt exact-tier matching, and `find_symbol` SHALL
NOT rank by BM25 relevance.

`search_symbols` SHALL accept a `pagination` argument carrying optional
`page_size` (rows per response) and optional `cursor`. The response SHALL NOT
contain a `total` field because today's `total` leaks the implementation's
over-fetch pool size and is meaningless to the agent.

#### Scenario: find_symbol covers a stack-trace identifier

- **WHEN** the agent calls `find_symbol(name: "OrderHandler")` with a name
  copied verbatim from a stack trace
- **THEN** the response items MUST include every symbol whose name contains
  `OrderHandler` (e.g. `OrderHandler`, `CancelOrderHandler`, `IOrderHandler`,
  `Foo.Bar.OrderHandler.Method`)
- **AND** the items MUST be ordered by `match_kind` first, then by `len(name)`
  ascending, then by `short_id` ascending

#### Scenario: search_symbols covers a natural-language intent

- **WHEN** the agent calls `search_symbols(query: "user registration")`
- **THEN** the response returns the first `page_size` symbols by blended BM25 +
  vector relevance, ordered by `score DESC, len(name) ASC, short_id ASC`
- **AND** the response MUST NOT contain a `total` field
- **AND** the cumulative result across all pages MUST NOT exceed 30 (the
  server's fixed top-K materialize cap)

## ADDED Requirements

### Requirement: kenn skill documents the page_size envelopes

The kenn skill at `claude-plugins/kenn/skills/kenn/SKILL.md` SHALL document the
pagination contract so the agent can adapt its calls without inspecting every
tool description individually. The documentation MUST state:

- `page_size` is the agent's rows-per-response choice, NOT a total budget.
- The per-family envelopes:
  iteration tools default page_size 25 / max 50,
  top-K relevance tools default page_size 10 / max 30.
- Top-K tools have a fixed server-side materialize cap of 30 results —
  the cursor walks within those 30, never beyond.
- Iteration tools have no server-side total cap — the cursor walks the full
  corpus until exhaustion.
- `nextCursor: null` signals "no more rows from this query." For top-K it
  means all 30 (or fewer) have been emitted; for iteration it means the
  corpus is exhausted.

#### Scenario: skill documents the envelope contract

- **WHEN** the agent invokes the `kenn` skill before a kenn session
- **THEN** the skill content MUST include a section that names the two
  envelope families with their concrete default and max page_size numbers
- **AND** the skill MUST state that `page_size` controls rows per response,
  not a total budget
- **AND** the skill MUST explain the top-K materialize cap (30) and that
  iteration tools have no such cap

### Requirement: page_size is the only pagination knob

Every paginated tool in `kenn-mcp` SHALL accept the agent's pagination input
as `pagination.page_size: Option<u32>` only. There SHALL NOT be a `limit`
parameter or any other "total budget" knob on the request side. The server
SHALL clamp `page_size` to the family's bounds and apply the family default
when omitted.

Note on the MCP pagination spec: the upstream spec
(`mcp-pagination-spec-alignment`, see archive) covers `tools/list` and the
other meta-operations but is **silent on tool-result pagination** and on
the shape of `limit`/`page_size` parameters. Kenn's `pagination` argument
and cursor envelope are kenn-specific extensions — the contract is defined
by this spec plus each tool's description string, not by the upstream
pagination spec.

The two families and their envelopes:

| Family | Default page_size | Max page_size | Total cap |
|---|---:|---:|---|
| Iteration tools (`list_*`, `find_*`, `find_similar`) | 25 | 50 | none (cursor walks the corpus) |
| Top-K relevance (`search_symbols`, `search_findings`, `semantic_search`) | 10 | 30 | 30 (fixed server-side materialize cap) |

The server SHALL emit `nextCursor: null` when there are no more rows to emit.
For top-K that means the cached materialized window is exhausted; for
iteration that means the corpus is exhausted.

If the agent passes a different `page_size` mid-walk on a top-K cursor, the
server SHALL honor it — re-slicing a cached top-K result at any page_size is
valid.

#### Scenario: page_size at default yields the default page

- **WHEN** the agent calls `search_symbols(query: "x")` with no page_size
- **THEN** the response items count is at most 10
- **AND** when the agent calls `list_callers(id: "rs:foo")` with no page_size
  the response items count is at most 25

#### Scenario: page_size is clamped to the family max

- **WHEN** the agent calls `search_symbols(query: "x", pagination: { page_size: 9999 })`
- **THEN** the response items count is at most 30
- **AND** when the agent calls `list_callers(id: "rs:foo", pagination: { page_size: 9999 })`
  the response items count is at most 50

#### Scenario: agent picks a tight page_size for a focused query

- **WHEN** the agent calls `search_symbols(query: "x", pagination: { page_size: 1 })`
- **THEN** the response contains exactly 1 item (assuming a match exists)
- **AND** a cursor is emitted (because there are more rows in the top-30 window)

#### Scenario: iteration tool walks the full corpus

- **WHEN** the agent calls `list_callers(id: "rs:hot_function")` against a
  symbol with 200 callers, walking the cursor to exhaustion
- **THEN** all 200 callers are emitted across the pages
- **AND** the server MUST NOT cap the cumulative emission below 200

### Requirement: search_symbols bounds the over-fetch pool

The `search_symbols` implementation SHALL bound its internal over-fetch pool
by a fixed ceiling that does NOT scale linearly with the materialize cap. The
over-fetch pool size SHALL NOT be exposed to callers. Per-query cost MUST
therefore be O(1) in the materialize cap past the ceiling. The implementation
MAY over-fetch up to the ceiling to give the merge step enough candidates for
stable ranking.

#### Scenario: pool size does not scale with materialize cap

- **WHEN** the implementation evaluates `search_symbols` for any legal
  materialize cap
- **THEN** the per-probe pool size MUST be at most 256

### Requirement: search_findings is top-K relevance for the findings store

The `search_findings` tool SHALL return ranked findings by BM25 over the
findings text, paginated under the top-K envelope (`page_size` default 10,
max 30; server materialize cap 30). The response SHALL NOT contain a `total`
field.

This requirement aligns `search_findings` with `search_symbols` and
`semantic_search`: all three tools surface top-K relevance results under the
same envelope.

#### Scenario: search_findings paginates under the top-K cap

- **WHEN** the agent calls `search_findings(query: "stale cursor")` with no
  page_size against a store with 200 matching findings
- **THEN** the response is a single page of up to 10 findings (default
  page_size 10)
- **AND** when the agent walks the cursor to exhaustion the cumulative items
  MUST NOT exceed 30 (the server materialize cap)
- **AND** no response MUST contain a `total` field
