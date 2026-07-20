## MODIFIED Requirements

### Requirement: trait surface in kenn-store::api

The `kenn-store` crate SHALL expose an `api` module containing the `Reader` trait plus the
value types storage operations work with. The ingestion-run lifecycle (`begin` /
`write_batch` / `end`) SHALL NOT be a `kenn-store` trait — it is owned by `kenn-indexer` (see
the `indexing-orchestrator` capability).

`Reader` SHALL be an async trait that covers every read operation the MCP reader consumes:
per-row symbol / def / def-line / file / package / symbol-docs fetchers; graph traversal
(`list_inbound`, `list_outbound`, `list_module_files`, `find_at_location`); text search
(`search_symbols_by_name`, `find_symbol_tiered`); hybrid search (`search_symbols_blended`);
and catalog queries (`distinct_languages`, `distinct_packages`, `count_table`).

`kenn-store` has a **single storage backend** built on **one storage engine, SQLite**. Every
database it persists — the code graph and the search / knowledge store — is a SQLite
database; no other storage engine is used. It SHALL expose that backend's ingestion,
aggregate, and finalize operations as public inherent methods on its concrete writer type,
and SHALL NOT wrap those operations in an ingestion-lifecycle trait. The concrete reader and
writer types SHALL be reachable as the crate-root `DbReader` / `DbWriter` types returned by
the `open_reader` / `open_writer` factory functions; callers SHALL obtain readers and writers
through those factories and SHALL NOT name a backend module path. Because the underlying
SQLite API is synchronous, the backend SHALL wrap blocking calls in `spawn_blocking` at the
`Reader` impl boundary so the async trait contract is preserved.

The `api` module SHALL also export:

- `WriteBatch` — value type accumulating per-table records, consumed by the backend's
  `write_batch` operation.
- The row and result types: `SymbolRow`, `DefRow`, `DefLineRow`, `PackageRow`, `FileRow`,
  `SymbolDocsRow`, `BlendedSymbolRow`, `FoundSymbolRow`, `RankedSymbolRow`, `MatchKind`,
  `WriterOptions`, `DbError`.

#### Scenario: indexer drives the backend through inherent operations

- **WHEN** `kenn-indexer` is built
- **THEN** each language ingester calls the backend's public inherent operations and
  `kenn_store::api::WriteBatch`
- **AND** no source file references a `kenn_store::api::Writer` ingestion-lifecycle trait

#### Scenario: mcp reader compiles against the Reader trait only

- **WHEN** `kenn-mcp` is built
- **THEN** all reader call sites obtain a reader via `kenn_store::open_reader` and call
  methods declared on `kenn_store::api::Reader`
- **AND** no `kenn-mcp` call site changes as a result of the engine swap

#### Scenario: the backend depends on no engine but SQLite

- **WHEN** `kenn-store` is built
- **THEN** no `lance*`, `datafusion*`, `arrow*`, `parquet`, `sqlparser`, or `object_store`
  crate appears in the `kenn-store` dependency tree
- **AND** the code graph and the search / knowledge store are both SQLite databases
