# storage-backend-abstraction

## Purpose

This spec defines the surface `kenn-store` exposes for read,
search, and graph operations: the `api::Reader` trait, the
`WriteBatch` value type, the row / result / error types, and the
`open_writer` / `open_reader` factories that hide the single
storage backend (SQLite) behind the `DbReader` / `DbWriter` types.
The ingestion-run lifecycle is not a `kenn-store` trait — it is
exposed as inherent operations on the concrete writer type and
driven by `kenn-indexer`.

Because the underlying SQLite API is synchronous, the backend wraps
blocking calls in `spawn_blocking` at the `Reader` impl boundary so
the async trait contract is preserved.
## Requirements
### Requirement: trait surface in kenn-store::api

The `kenn-store` crate SHALL expose an `api` module containing the `Reader` trait plus the value types storage operations work with. The ingestion-run lifecycle (`begin` / `write_batch` / `end`) SHALL NOT be a `kenn-store` trait — it is owned by `kenn-indexer` (see the `indexing-orchestrator` capability).

`Reader` SHALL be an async trait that covers every read operation the MCP reader consumes: per-row symbol / def / def-line / file / package / symbol-docs fetchers; graph traversal (`list_inbound`, `list_outbound`, `list_module_files`, `find_at_location`); text search (`search_symbols_by_name`, `find_symbol_tiered`); hybrid search (`search_symbols_blended`); and catalog queries (`distinct_languages`, `distinct_packages`, `count_table`).

`kenn-store` has a **single storage backend** built on **one storage engine, SQLite**. Every database it persists — the code graph and the search / knowledge store — is a SQLite database; no other storage engine is used. It SHALL expose that backend's ingestion, aggregate, and finalize operations as public inherent methods on its concrete writer type, and SHALL NOT wrap those operations in an ingestion-lifecycle trait. The concrete reader and writer types SHALL be reachable as the crate-root `DbReader` / `DbWriter` types returned by the `open_reader` / `open_writer` factory functions; callers SHALL obtain readers and writers through those factories and SHALL NOT name a backend module path. Because the underlying SQLite API is synchronous, the backend SHALL wrap blocking calls in `spawn_blocking` at the `Reader` impl boundary so the async trait contract is preserved.

The `api` module SHALL also export:

- `WriteBatch` — value type accumulating per-table records, consumed by the backend's `write_batch` operation.
- The row and result types: `SymbolRow`, `DefRow`, `DefLineRow`, `PackageRow`, `FileRow`, `SymbolDocsRow`, `BlendedSymbolRow`, `FoundSymbolRow`, `RankedSymbolRow`, `MatchKind`, `WriterOptions`, `DbError`.

#### Scenario: indexer drives the backend through inherent operations

- **WHEN** `kenn-indexer` is built
- **THEN** each language ingester calls the backend's public inherent operations and `kenn_store::api::WriteBatch`
- **AND** no source file references a `kenn_store::api::Writer` ingestion-lifecycle trait

#### Scenario: mcp reader compiles against the Reader trait only

- **WHEN** `kenn-mcp` is built
- **THEN** all reader call sites obtain a reader via `kenn_store::open_reader` and call methods declared on `kenn_store::api::Reader`
- **AND** no `kenn-mcp` call site changes as a result of the engine swap

#### Scenario: the backend depends on no engine but SQLite

- **WHEN** `kenn-store` is built
- **THEN** no `lance*`, `datafusion*`, `arrow*`, `parquet`, `sqlparser`, or `object_store` crate appears in the `kenn-store` dependency tree
- **AND** the code graph and the search / knowledge store are both SQLite databases

### Requirement: hybrid search is encapsulated behind one trait method

`Reader::search_symbols_blended` SHALL accept a query payload including text + optional vector + tunable parameters and SHALL return a single ranked `Vec<BlendedSymbolRow>`.

`Reader` SHALL NOT expose the BM25 result list or vector kNN result list separately to the caller. Fusion (native blend, RRF, weighted, or otherwise) SHALL be the backend's choice.

#### Scenario: caller cannot observe fusion mechanism

- **WHEN** `kenn-mcp` invokes `search_symbols_blended` against the active reader
- **THEN** it receives a single ranked list of `BlendedSymbolRow`
- **AND** no public method on `Reader` returns the unfused BM25 or vector candidates

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

