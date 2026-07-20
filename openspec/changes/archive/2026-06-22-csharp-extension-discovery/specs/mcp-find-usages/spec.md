## ADDED Requirements

### Requirement: A type's extension methods are reachable via `extends_type`

`find_usages` and `list_usages` SHALL accept `extends_type` as a queryable
incoming edge kind, so that requesting the `extends_type` edges of a type returns
the extension methods that extend it. The augmenting methods are surfaced as
incoming edges on the target type; no new tool is required.

#### Scenario: list the extension methods of a type

- **WHEN** `find_usages` is called on `Order` with `edge_kinds` including
  `extends_type`
- **THEN** the result includes every extension method that extends `Order`
- **AND** excludes ordinary methods that merely take an `Order` argument
