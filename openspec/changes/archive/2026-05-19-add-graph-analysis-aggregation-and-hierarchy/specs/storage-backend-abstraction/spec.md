## ADDED Requirements

### Requirement: Reader exposes scan_aggregate_nodes

The `api::Reader` trait SHALL define an async method:

```rust
fn scan_aggregate_nodes(
    &self,
) -> impl Future<Output = Result<Vec<AggregateNodeRow>, DbError>> + Send;
```

`AggregateNodeRow` is the public row type for an aggregate node, mirroring the persisted `AggregateNodeRecord` fields with the `kind` and `language` enums rendered to their `db_name()` strings (consistent with the existing `SymbolRow` shape).

A backend that does not implement aggregated graph storage SHALL return `Err(DbError::Backend("scan_aggregate_nodes: not implemented in <backend>"))`. A backend that supports the tables but encounters an empty snapshot (e.g. older schema) SHALL return `Ok(Vec::new())` without error.

#### Scenario: Default backend returns rows for a current snapshot

- **WHEN** `scan_aggregate_nodes` runs against a snapshot indexed with the aggregate tables present
- **THEN** the result MUST be `Ok(rows)` with one entry per aggregate node

#### Scenario: Default backend returns empty for an older snapshot

- **WHEN** `scan_aggregate_nodes` runs against a snapshot whose `aggregate_nodes` table is absent
- **THEN** the result MUST be `Ok(Vec::new())`

#### Scenario: Legacy backend reports unsupported

- **WHEN** `scan_aggregate_nodes` runs against the legacy surreal backend
- **THEN** the result MUST be `Err(DbError::Backend(_))` whose message indicates the operation is not implemented

### Requirement: Reader exposes scan_aggregate_edges

The `api::Reader` trait SHALL define an async method:

```rust
fn scan_aggregate_edges(
    &self,
) -> impl Future<Output = Result<Vec<AggregateEdgeRow>, DbError>> + Send;
```

`AggregateEdgeRow` carries `src_short_id: u32`, `tgt_short_id: u32` (with `src <= tgt` enforced at the row layer for undirected semantics), `kind: String` (the edge-kind `db_name`), and `weight: u32`.

The same empty / unsupported behavior as `scan_aggregate_nodes` applies.

#### Scenario: Default backend returns sorted-endpoint rows

- **WHEN** `scan_aggregate_edges` runs against a snapshot whose `aggregate_edges` table contains an edge between aggregates 10 and 5 (kind `calls`, weight 6)
- **THEN** the result MUST contain a row `{ src_short_id: 5, tgt_short_id: 10, kind: "calls", weight: 6 }`

#### Scenario: Multiple kinds between same pair produce distinct rows

- **WHEN** aggregates 5 and 10 are connected by both `calls` (weight 3) and `type_use` (weight 2)
- **THEN** the result MUST contain exactly two rows, one per kind

### Requirement: Aggregate scans are not on the MCP hot path

The new scan methods SHALL be designed for the analyzer's bulk-load use case, not the MCP per-request hot path. Implementations MAY return a single `Vec` materialized in memory rather than streaming. Callers requiring incremental processing of very large aggregated graphs MUST use a backend-specific extension or paginate by post-processing.

#### Scenario: Default backend materializes the full result

- **WHEN** `scan_aggregate_nodes` is called against a snapshot with N aggregate nodes
- **THEN** the returned `Vec` MUST contain all N rows
