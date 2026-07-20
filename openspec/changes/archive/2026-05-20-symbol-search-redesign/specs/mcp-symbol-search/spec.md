## ADDED Requirements

### Requirement: Two distinct symbol-search tools partition the search space

The MCP server SHALL expose two symbol-search tools whose contracts make their
intended use cases mutually exclusive:

- `find_symbol(name, kind?, limit?, include_tests?, include_external?)` — for
  the **identifier** case: the agent has a literal name (from a stack trace,
  task description, prior tool output) and wants exact / near-exact matches.
- `search_symbols(query, filters?, pagination?, count_only?)` — for the
  **intent** case: the agent has a natural-language phrase or a loosely-recalled
  term and wants ranked relevance over symbol names AND documentation.

The MCP descriptions of both tools SHALL state which case each is for and
explicitly steer the agent toward the other tool for the opposite case.
`search_symbols` SHALL NOT attempt exact-tier matching, and `find_symbol` SHALL
NOT rank by BM25 relevance.

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
- **THEN** the response items MUST include symbols whose names match `user` or
  `registration` under the n-gram name index
- **AND** the items MUST also include symbols whose docstrings match either
  word under the stemmed documentation index
- **AND** the items MUST be ranked by a blended score that weights name
  matches above doc matches

### Requirement: find_symbol returns match_kind on every result

Each row of `find_symbol`'s `items` array SHALL carry a `match_kind` field with
one of four values, in this match-tier order:

1. `"exact"` — the symbol's `name` equals the query (case-insensitive),
   resolved via the redb `SYMBOLS_BY_NAME` key.
2. `"prefix"` — `name` starts with the query and is not exact, resolved via a
   redb key range scan.
3. `"contains"` — `name` contains the query as a substring (not a prefix),
   surfaced by the Lance n-gram name index.
4. `"fuzzy"` — the Lance n-gram index surfaced the row but `name` does not
   contain the query as a substring (e.g. query `"order handler"` against
   `Foo.Bar.OrderHandler.M`).

`find_symbol` SHALL order results by `match_kind` (in the order above), then by
`len(name)` ascending, then by `short_id` ascending.

#### Scenario: Match-tier ordering on a compound query

- **WHEN** the agent calls `find_symbol(name: "OrderHandler")` and the
  workspace contains `OrderHandler`, `CancelOrderHandler`, and
  `Foo.OrderHandler.M`, plus a documented method `RegisterOrder` whose
  docstring reads "this handles orders"
- **THEN** items[0] MUST be `OrderHandler` with `match_kind: "exact"`
- **AND** the next item MUST be the shortest `*OrderHandler` prefix or
  substring match
- **AND** `RegisterOrder` MUST NOT appear in the response — `find_symbol` is
  not a documentation search

#### Scenario: fuzzy tier matches a token split with no substring

- **WHEN** the agent calls `find_symbol(name: "order handler")` (with a space)
  and the workspace contains `Foo.Bar.OrderHandler.M`
- **THEN** the response MUST include `Foo.Bar.OrderHandler.M` with
  `match_kind: "fuzzy"`

### Requirement: search_symbols ranks by blended name + doc score

`search_symbols` SHALL query two Lance inverted indexes:

1. the n-gram name index, yielding a per-symbol `name_bm25` score;
2. the stemmed documentation index, yielding a per-symbol `doc_bm25` score.

It SHALL merge the two result sets by `short_id` (keeping the maximum score
when a symbol surfaces more than once per index) and compute a final score per
row as:

```
score = 3.0 * name_bm25
      + 1.0 * doc_bm25
      + 5.0 * (1 if lower(name) contains lower(query) else 0)
```

When two rows tie on `score`, the tiebreak SHALL be `len(name)` ascending, then
`short_id` ascending. The substring bonus exists to keep an obviously-named
symbol ahead of vague token-only matches when BM25's IDF is degenerate (the
case for every common code identifier).

Each result row SHALL carry `name_score`, `doc_score`, and the composite
`score` so the agent can see why a row ranked where it did.

#### Scenario: Substring bonus elevates the obvious answer

- **WHEN** the agent calls `search_symbols(query: "OrderHandler")` and the BM25
  name score is near zero across all rows (common-token IDF degeneracy)
- **THEN** rows whose `name` contains `OrderHandler` as a substring MUST appear
  before rows that surface only via n-gram fragments

#### Scenario: Doc-only matches included with lower weight

- **WHEN** the agent calls `search_symbols(query: "registration")` and no
  symbol name contains `registration` but several docstrings do
- **THEN** the response MUST still include the doc-matched symbols
- **AND** each such row's `score` MUST equal `1.0 * doc_score` (no name bonus)

#### Scenario: Shorter name wins a score tie

- **WHEN** two result rows have an equal blended `score`
- **THEN** the row with the shorter `name` MUST rank first
- **AND** if their names are also equal length, the lower `short_id` MUST rank
  first

### Requirement: search_symbols paginates with a stable cursor

`search_symbols` SHALL support cursor pagination. The cursor SHALL encode
`(snapshot_id, last_short_id)` — the existing 14-character list cursor. The
result order `(score DESC, len(name) ASC, short_id ASC)` is a total order, and
the globally-unique `short_id` alone pinpoints the boundary row; a score
component in the cursor could not resolve ties between rows of equal score, so
none is carried.

A page returned for a given cursor SHALL contain only rows that fall strictly
after the cursor's boundary row under `(score DESC, len(name) ASC,
short_id ASC)`. Consecutive pages SHALL therefore neither repeat nor skip a
row. When the cursor's `snapshot_id` no longer matches the active snapshot, the
server SHALL signal a stale cursor and pagination SHALL restart from the first
page.

`find_symbol` SHALL NOT accept a pagination cursor — its result set is small by
design and its ranking is fully determined by `(match_kind, len(name),
short_id)`.

#### Scenario: search_symbols cursor produces a gap-free continuation

- **WHEN** the agent issues `search_symbols(query: "order")` and then passes
  `pagination.cursor = response.next` to fetch the second page
- **THEN** the second page MUST contain only rows ranked strictly after the
  last row of page 1 under the documented blend ordering
- **AND** no row MUST appear in both pages

#### Scenario: Stale cursor restarts pagination

- **WHEN** the agent passes a `pagination.cursor` whose `snapshot_id` does not
  match the active snapshot
- **THEN** the server MUST signal a stale cursor rather than returning rows
  from a mismatched snapshot

### Requirement: find_symbol respects kind and limit constraints

`find_symbol` SHALL accept the standard `kind?` filter (array of `Kind` values)
and a `limit?` parameter (default 20, hard cap 200). A `limit` above the cap
SHALL be clamped to 200, not rejected.

#### Scenario: Kind filter narrows results

- **WHEN** the agent calls `find_symbol(name: "Order", kind: ["class"])`
- **THEN** every item MUST have `kind = "class"`

#### Scenario: limit caps at 200

- **WHEN** the agent calls `find_symbol(name: "Get", limit: 1000)`
- **THEN** the response MUST cap items at 200

### Requirement: BREAKING removal of search_by_intent

The MCP tool `search_by_intent` SHALL NOT be registered. Its documentation-only
search behavior is subsumed by `search_symbols`, whose blend already weighs
docstring relevance. Agents that previously called `search_by_intent(query: $q)`
SHALL be migrated to `search_symbols(query: $q)`.

#### Scenario: search_by_intent no longer registered

- **WHEN** a client issues `tools/list`
- **THEN** the response MUST NOT contain a tool named `search_by_intent`
- **AND** the response MUST contain tools named `find_symbol` and
  `search_symbols`
