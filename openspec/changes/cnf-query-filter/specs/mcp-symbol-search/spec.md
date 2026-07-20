## ADDED Requirements

### Requirement: Optional CNF lexical-filter query mode

The symbol-search tool SHALL support an optional structured query mode,
expressed as an array of token groups interpreted as conjunctive normal form: OR
within a group (alternatives/synonyms), AND between groups (required concepts).
When supplied, it SHALL act as a precision filter over the lexical arms —
narrowing results to those touching every group — and SHALL NOT change the
default ranking path. It SHALL relax (drop or OR a group) rather than return an
empty result set. A plain-string query SHALL remain the default and keep the RRF
ranking behavior unchanged.

#### Scenario: structured query narrows by required concepts

- **GIVEN** a structured query `[[cancel, abort], [order, purchase]]`
- **WHEN** the search runs
- **THEN** results must match `(cancel OR abort) AND (order OR purchase)` over
  the lexical arms

#### Scenario: over-constrained structured query relaxes instead of returning empty

- **GIVEN** a structured query whose AND-of-groups matches nothing
- **WHEN** the search runs
- **THEN** the filter relaxes (drops or ORs a group) and returns the closest
  matches rather than an empty set

#### Scenario: default query is unaffected

- **GIVEN** a plain-string query (no groups)
- **WHEN** the search runs
- **THEN** the default RRF ranking behavior is used unchanged
