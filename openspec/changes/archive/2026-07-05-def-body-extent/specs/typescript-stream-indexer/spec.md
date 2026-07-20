## ADDED Requirements

### Requirement: symbol frames carry a body range for the whole declaration

A `symbol` frame SHALL carry an optional `body` range (4-int, 0-based, same
convention as the existing name-span `range`) giving the full declaration span —
the whole function/class/interface, including its leading doc comment. It SHALL
be sourced from the declaration node (`rangeOf(sf, decl)`), not from the name
node (`rangeOf(sf, nameNode)`, which drives the existing name span).

When a symbol's declaration span is unavailable, the `body` field SHALL be
omitted; ingest treats an absent `body` as a `0` def body extent and
`get_source` falls back to the name span.

#### Scenario: a function emits a body range spanning its declaration

- **WHEN** a function's name is on file line 12 and its declaration (JSDoc
  through closing brace) spans file lines 10–20 (0-based 9–19)
- **THEN** the `symbol` frame's name `range` MUST be the name span at line 12
- **AND** its `body` MUST be `[9, …, 19, …]` (0-based, the whole declaration)
