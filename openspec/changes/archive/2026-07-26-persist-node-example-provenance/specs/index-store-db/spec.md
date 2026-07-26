## MODIFIED Requirements

### Requirement: aggregate_nodes table

The default-backend schema SHALL define an `aggregate_nodes` SQLite table with one row per aggregate node:

```
aggregate_nodes
  short_id     u32      // same id as the underlying anchor symbol
  kind         string   // class / struct / trait / interface / enum / module / namespace / package
  name         string   // display name
  language     string
  external     bool     // mirrors the anchor symbol's external
  test         bool     // mirrors the anchor symbol's test
  example      bool     // the node's primary def lies under an example/sample/demo/fixture path
  anchor_id    u32      // interned anchor id (package short_id, or interned path-prefix id)
  anchor_name  string   // human-readable anchor label
```

The aggregate node's `short_id` SHALL be the `short_id` of the anchor symbol the projection rolled it up to (the nearest enclosing class-like or module-like symbol). This means aggregate nodes are a subset of `symbols`, and the same id space identifies a symbol and its corresponding aggregate node when one exists.

`external`, `test`, and `example` are the node's provenance flags. `example` SHALL be evaluated once, during aggregation, from the node's primary definition file path — the same path the anchor resolution already reads — and SHALL NOT be re-derived by consumers. A consumer that needs to know whether a node is example code SHALL read this column.

#### Scenario: Aggregated class is queryable by short_id

- **WHEN** a class with `short_id = 42` is the aggregate target for some method's roll-up
- **THEN** the `aggregate_nodes` table MUST contain a row with `short_id = 42`
- **AND** that row's `kind` MUST be `class`

#### Scenario: Symbol not chosen as any aggregate is absent

- **WHEN** a method `short_id = 99` rolls up to its enclosing class `42`
- **THEN** the `aggregate_nodes` table MUST NOT contain a row with `short_id = 99`

#### Scenario: A node defined under an example path is flagged

- **WHEN** an aggregate node's primary definition file is `crates/<pkg>/examples/spike.rs`
- **THEN** its `aggregate_nodes` row MUST have `example = true`
- **AND** a node defined under `crates/<pkg>/src/` MUST have `example = false`

#### Scenario: A snapshot written before the column is rejected, not misread

- **WHEN** a snapshot persisted under an older store schema version is opened
- **THEN** the store MUST report a schema mismatch and require a reindex
- **AND** MUST NOT surface a partially-read `aggregate_nodes` row
