## MODIFIED Requirements

### Requirement: find_symbol returns match_kind on every result

Each row of `find_symbol`'s `items` array SHALL carry a `match_kind` field with
one of four values, in this match-tier order:

1. `"exact"` — the symbol's `name` equals the query (case-insensitive),
   resolved via an equality query on the Lance scalar BTREE index over the
   symbol-name column.
2. `"prefix"` — `name` starts with the query and is not exact, resolved via a
   range query on that same BTREE index.
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
