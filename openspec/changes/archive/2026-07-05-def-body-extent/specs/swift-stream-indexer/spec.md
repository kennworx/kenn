## ADDED Requirements

### Requirement: symbol frames carry a body range for the whole declaration

A `symbol` frame SHALL carry an optional `body` range (4-int, 0-based, same
convention as the name-span `range`) giving the full declaration span — the
whole `class`/`struct`/`enum`/`protocol`/`func`/… including its attributes,
through the closing brace. Because libIndexStore occurrences are point-based (a
name location, no extent), the span SHALL be recovered by parsing the source
file with **SwiftSyntax** and mapping the declaration's name-token line to the
node span (`positionAfterSkippingLeadingTrivia` → `endPositionBeforeTrailing-
Trivia`, i.e. attributes included, leading doc comment excluded).

When the file cannot be parsed, or no declaration name lands on the definition's
line, the `body` field SHALL be omitted; ingest treats an absent `body` as a `0`
def body extent and `get_source` falls back to the name span. Each def-bearing
file SHALL be parsed at most once per run.

#### Scenario: a struct emits a body range spanning its declaration

- **WHEN** a Swift `struct Order` is declared on file line 5 and its closing
  brace is on line 10
- **THEN** the `symbol` frame's `range` MUST be the name span at line 5 (0-based
  `[4, 0, 4, 0]`)
- **AND** its `body` MUST be `[4, 0, 9, 0]` (0-based, the whole declaration)

#### Scenario: an unparseable file omits the body range

- **WHEN** a definition's source file cannot be read or parsed
- **THEN** the `symbol` frame MUST omit `body`, and `get_source` returns the
  declaration line
