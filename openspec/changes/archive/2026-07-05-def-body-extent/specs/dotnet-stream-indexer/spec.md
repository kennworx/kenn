## ADDED Requirements

### Requirement: symbol frames carry a body range for the whole declaration

A `symbol` frame SHALL carry an optional `body` range (4-int, 0-based, same
convention as the existing `def_range` name span) giving the full declaration
node span — the whole `class`/`method`/`property` body, including its leading
doc comment and attributes. It SHALL be sourced from the declaration syntax node
(`ISymbol.DeclaringSyntaxReferences[0].GetSyntax()` →
`RangeUtil.FromSyntaxNode(node)`), not from `ISymbol.Locations` (which is the
name identifier and drives the existing name span).

When a symbol has no declaring syntax (metadata-only / external), the `body`
field SHALL be omitted; ingest treats an absent `body` as a `0` def body extent
and `get_source` falls back to the name span.

#### Scenario: a method emits a body range spanning its declaration

- **WHEN** a C# method's name identifier is on file line 41 and its declaration
  (attributes through closing brace) spans file lines 39–58 (0-based 38–57)
- **THEN** the `symbol` frame's `def_range` MUST be the name span at line 41
- **AND** its `body` MUST be `[38, …, 57, …]` (0-based, the whole declaration)

#### Scenario: a metadata-only symbol omits the body range

- **WHEN** an external/metadata symbol has no declaring syntax reference
- **THEN** the `symbol` frame MUST omit `body`
