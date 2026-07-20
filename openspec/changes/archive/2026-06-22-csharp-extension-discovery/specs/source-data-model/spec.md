## ADDED Requirements

### Requirement: The graph models an `extends_type` augmentation edge

The data model SHALL define an `extends_type` edge kind whose source is a symbol
that augments a type from outside the type's own declaration (e.g. a C# extension
method) and whose target is the type being augmented. The edge is
**non-containment**: it SHALL NOT replace or duplicate the source symbol's
`defined_in` edge, which continues to point at the symbol's real declaring scope.
A type's augmenting symbols are its **incoming** `extends_type` edges. The kind
parallels the existing `extends_rule` (stylesheet `@extend`) — a non-containment
"extends" relation — and SHALL serialize as the string `extends_type` identically
on the JSONL wire and in the model.

#### Scenario: an extension method gains an edge to the type it extends

- **WHEN** a C# extension method `Foo` declared in `OrderExtensions` extends
  `Order`
- **THEN** the graph contains an `extends_type` edge from `Foo` to `Order`
- **AND** `Foo` retains its `defined_in` edge to `OrderExtensions`

#### Scenario: the augmented type lists its augmenting symbols

- **WHEN** `Order` is the receiver of two extension methods
- **THEN** `Order` has exactly two incoming `extends_type` edges, one per method

#### Scenario: the edge string is stable across wire and model

- **WHEN** an `extends_type` edge is serialized on the JSONL wire and parsed into
  the model
- **THEN** both spell the kind `extends_type`
