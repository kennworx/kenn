## ADDED Requirements

The two new tables below persist the "aggregated graph" defined in the
`graph-analysis` capability: a weighted undirected projection of the
per-symbol graph in which non-grouping symbols (methods, fields,
parameters, free functions, …) are rolled up to their nearest enclosing
class-like or module-like symbol. Each `aggregate_node` row corresponds
to one such anchor symbol; each `aggregate_edges` row corresponds to one
undirected weighted edge between two anchors for a specific edge kind.
See `specs/graph-analysis/spec.md` for the projection rules and per-kind
weights; this spec only fixes the on-disk shape.

### Requirement: aggregate_nodes table

The default-backend schema SHALL define an `aggregate_nodes` table keyed by aggregate `short_id` (`u32` big-endian) with a bincode-encoded `AggregateNodeRecord` value carrying:

```
AggregateNodeRecord
  short_id      u32      // same id as the underlying anchor symbol
  kind          Kind     // class / struct / trait / interface / enum / module / namespace / package
  name          String   // display name
  language      Language
  external      bool     // mirrors SymbolRecord.external for the anchor
  test          bool     // mirrors SymbolRecord.test for the anchor
  anchor_id     u32      // interned anchor id (package short_id, or interned path-prefix id)
  anchor_name   String   // human-readable anchor label
```

The aggregate node's `short_id` SHALL be the `short_id` of the anchor symbol the projection rolled it up to (the nearest enclosing class-like or module-like symbol). This means aggregate nodes are a subset of `symbols`, and the same id space identifies a symbol and its corresponding aggregate node when one exists.

#### Scenario: Aggregated class is queryable by short_id

- **WHEN** a class with `short_id = 42` is the aggregate target for some method's roll-up
- **THEN** the `aggregate_nodes` table MUST contain a row keyed by `u32_be(42)`
- **AND** that row's `kind` MUST be `class`

#### Scenario: Symbol not chosen as any aggregate is absent

- **WHEN** a method `short_id = 99` rolls up to its enclosing class `42`
- **THEN** the `aggregate_nodes` table MUST NOT contain a row keyed by `u32_be(99)`

### Requirement: aggregate_edges table

The default-backend schema SHALL define an `aggregate_edges` table keyed by `pair_u32_be(min(src, tgt), max(src, tgt)) ++ u32_be(EdgeKind as u32)` (12 bytes) with a `u32_be(weight)` value. Each row represents one undirected aggregated edge of a specific kind between two aggregate nodes. Multiple kinds between the same pair of aggregates SHALL be stored as separate rows (different key suffix).

Sorted endpoints (min then max) SHALL ensure undirected deduplication at write time — there MUST NOT be two rows for the same `(a, b, kind)` differing only by direction.

#### Scenario: Symmetric edge writes deduplicate at the table layer

- **WHEN** the aggregation pass produces a `calls` edge between aggregates 5 and 10 (regardless of which is source and which is target)
- **THEN** the `aggregate_edges` table MUST contain exactly one row keyed by `pair_u32_be(5, 10) ++ u32_be(calls as u32)`

#### Scenario: Multiple kinds between same pair produce separate rows

- **WHEN** aggregates 5 and 10 are connected by both `calls` (weight 3) and `type_use` (weight 2)
- **THEN** the table MUST contain two rows: one keyed with the `calls` suffix and value `u32_be(3)`, one with the `type_use` suffix and value `u32_be(2)`

### Requirement: Aggregate tables written during end_run

The indexer SHALL populate `aggregate_nodes` and `aggregate_edges` inside `end_run`, after all per-unit ingest is complete (all symbols, edges, and defs already in their tables), and before the snapshot is published as live. A failure during aggregate writes SHALL fail `end_run` and prevent snapshot publication — partial aggregate tables MUST NOT be observable to readers.

#### Scenario: Successful end_run publishes a snapshot with non-empty aggregate tables

- **WHEN** `kenn index` completes successfully on a workspace with at least one class and one inter-class call
- **THEN** the published snapshot's `aggregate_nodes` table MUST be non-empty
- **AND** the `aggregate_edges` table MUST be non-empty

#### Scenario: Aggregate-write failure prevents snapshot publication

- **WHEN** aggregate writes fail mid-`end_run` (simulated I/O error)
- **THEN** the previous live snapshot MUST remain unchanged
- **AND** the new snapshot's building directory MUST NOT be flipped to live

### Requirement: Schema version bump

The default-backend `SCHEMA_VERSION` constant SHALL be bumped from `1` to `2` to mark the addition of the aggregate tables. Readers SHALL accept snapshots at either version. Snapshots at version 1 lack the aggregate tables; reads of `scan_aggregate_nodes` / `scan_aggregate_edges` on such snapshots return empty rather than error.

#### Scenario: Reader serves a version 1 snapshot

- **WHEN** a kenn binary built for version 2 opens a snapshot persisted under version 1
- **THEN** the reader MUST open successfully
- **AND** `scan_aggregate_nodes` and `scan_aggregate_edges` MUST return empty vectors
