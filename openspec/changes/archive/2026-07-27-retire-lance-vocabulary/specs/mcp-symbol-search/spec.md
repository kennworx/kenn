## MODIFIED Requirements

### Requirement: find_symbol returns match_kind on every result

Each row of `find_symbol`'s `items` array SHALL carry a `match_kind` field with
one of four values, in this match-tier order:

1. `"exact"` — the symbol's `name` equals the query (case-insensitive).
2. `"prefix"` — `name` starts with the query and is not exact.
3. `"contains"` — `name` contains the query as a substring (not a prefix).
4. `"fuzzy"` — the candidate scan surfaced the row but `name` does not
   contain the query as a substring (e.g. query `"order handler"` against
   `Foo.Bar.OrderHandler.M`).

`match_kind` is a **classification of the result**, not a statement about which
index produced it: all four tiers are classified over one candidate set, drawn
from the FTS5 **trigram** index over the symbol-name column. The trigram
tokenizer requires at least three alphanumeric characters, so a shorter query
SHALL yield no candidates rather than a degraded scan.

`find_symbol` SHALL order results by `match_kind` (in the order above), then by
`len(name)` ascending, then by `short_id` ascending.

#### Scenario: Match-tier ordering on a compound query

- **WHEN** the agent calls `find_symbol(name: "OrderHandler")` and the
  workspace contains an exact `OrderHandler`, a prefixed
  `OrderHandlerFactory`, and a qualified `Foo.Bar.OrderHandler.M`
- **THEN** the exact match sorts before the prefix match, which sorts before
  the substring match
- **AND** each row reports the `match_kind` that placed it

#### Scenario: fuzzy tier matches a token split with no substring

- **WHEN** the agent calls `find_symbol(name: "order handler")` (with a space)
  and the workspace contains `Foo.Bar.OrderHandler.M`
- **THEN** the response MUST include `Foo.Bar.OrderHandler.M` with
  `match_kind: "fuzzy"`

#### Scenario: a query too short for the trigram tokenizer

- **WHEN** the agent calls `find_symbol` with a query of fewer than three
  alphanumeric characters
- **THEN** no candidates are produced
- **AND** the call succeeds with an empty result rather than scanning every
  symbol
