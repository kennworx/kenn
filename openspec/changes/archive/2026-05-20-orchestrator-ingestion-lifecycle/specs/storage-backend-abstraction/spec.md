## MODIFIED Requirements

### Requirement: trait surface in kenn-store::api

The `kenn-store` crate SHALL expose an `api` module containing the `Reader` trait plus the value types storage operations work with. The ingestion-run lifecycle (`begin` / `write_batch` / `end`) SHALL NOT be a `kenn-store` trait — it is owned by `kenn-indexer` (see the `indexing-orchestrator` capability).

`Reader` SHALL be an async trait that covers every read operation the MCP reader consumes: per-row symbol / def / def-line / file / package / symbol-docs fetchers; graph traversal (`list_inbound`, `list_outbound`, `list_module_files`, `find_at_location`); text search (`search_symbols_by_name`, `find_symbol_tiered`); hybrid search (`search_symbols_blended`); and catalog queries (`distinct_languages`, `distinct_packages`, `count_table`).

`kenn-store` has a **single storage backend** (Lance + redb). It SHALL expose that backend's ingestion, aggregate, and finalize operations as public inherent methods on its concrete writer type, and SHALL NOT wrap those operations in an ingestion-lifecycle trait. The concrete reader and writer types SHALL be reachable as the `ActiveReader` / `ActiveWriter` aliases returned by the `open_reader` / `open_writer` factory functions; callers SHALL obtain readers and writers through those factories and SHALL NOT name a backend module path.

The `api` module SHALL also export:

- `WriteBatch` — value type accumulating per-table records, consumed by the backend's `write_batch` operation.
- The row and result types: `SymbolRow`, `DefRow`, `DefLineRow`, `PackageRow`, `FileRow`, `SymbolDocsRow`, `EdgeRow`, `BlendedSymbolRow`, `FoundSymbolRow`, `RankedSymbolRow`, `MatchKind`, `WriterOptions`, `DbError`.

The `BatchingWriter` adapter SHALL be removed; record batching is done inline by the `kenn-indexer` DB-writer thread as it drains the ingester record channel.

#### Scenario: indexer drives the backend through inherent operations

- **WHEN** `kenn-indexer` is built
- **THEN** the DB-writer thread calls the backend's public inherent operations and `kenn_store::api::WriteBatch`
- **AND** no source file references a `kenn_store::api::Writer` ingestion-lifecycle trait or `kenn_store::api::BatchingWriter`

#### Scenario: mcp reader compiles against the Reader trait only

- **WHEN** `kenn-mcp` is built
- **THEN** all reader call sites obtain a reader via `kenn_store::open_reader` and call methods declared on `kenn_store::api::Reader`

#### Scenario: kenn-store exposes no ingestion-lifecycle trait

- **WHEN** `kenn-store` is built
- **THEN** its `api` module declares `Reader` but no `Writer` ingestion-lifecycle trait
- **AND** the backend's ingestion operations are reachable as public inherent methods on `ActiveWriter`

### Requirement: trait does NOT promise cross-engine atomicity

The backend's `write_batch` and finalize operations SHALL NOT guarantee cross-index atomicity. The backend is composed of two internal stores (redb and Lance); a write MAY land in one and not the other.

A reader observing a snapshot after a partial-flush crash MAY see some indices populated and others empty for the same logical batch.

The backend SHALL NOT expose any cross-reader/writer transaction handle.

The documented recovery posture SHALL be re-ingest from the source corpus.

#### Scenario: backend docs state non-atomicity

- **WHEN** `cargo doc -p kenn-store` is generated
- **THEN** the `write_batch` and finalize operation doc comments state that cross-engine atomicity is not guaranteed and that recovery from a partial flush is caller-driven re-ingest

## REMOVED Requirements

### Requirement: writer trait keeps the existing batched-flush shape

**Reason**: The ingestion-run lifecycle (`begin` / `write_batch` / `end`) is the ingester's state machine, not a storage concern — and with a single compile-time backend it needs no trait at all.

**Migration**: The ingester→writer seam becomes a bounded record channel; there is no ingestion-lifecycle trait and no `BatchingWriter` (see the `indexing-orchestrator` capability). The `kenn-store` backend exposes `write_batch` and the rest of the ingestion lifecycle as public inherent methods on the concrete writer type; the `kenn-indexer` DB-writer thread drains the channel, batches inline, and calls those methods directly.

### Requirement: compile-time backend selection via cargo features

**Reason**: The project has a single storage backend (Lance + redb). The legacy `SurrealDB` backend was removed, and with it the `db_default` / `db_surreal` cargo features — there is nothing to select between, so feature-gated backend selection no longer exists.

**Migration**: `open_writer` / `open_reader` are plain factory functions over the one backend; `ActiveWriter` / `ActiveReader` are direct type aliases for its concrete types. Callers are unaffected — they already obtained writers and readers through the factories. The `kenn-store` crate has no `[features]` table; the backend lives under the private `db` module.

### Requirement: SurrealDB code SHALL live behind the abstraction

**Reason**: The `SurrealDB` backend was removed entirely. There is no `SurrealDB` code left to encapsulate.

**Migration**: `surrealdb`-dependent code, the `db_surreal` module, and the `surrealdb` dependency were deleted. The single Lance + redb backend lives under the private `kenn-store::db` module and is reached only through `open_writer` / `open_reader`.

### Requirement: cross-backend correctness fixture harness

**Reason**: With a single backend there is no second backend to verify result-parity against; a cross-backend harness is meaningless.

**Migration**: The fixture corpus and assertions are retained as an ordinary single-backend correctness test (`kenn-store/tests/storage_fixtures.rs`) — they still exercise the backend, they just no longer compare two backends.

### Requirement: cross-backend perf bench harness

**Reason**: With a single backend there is no cross-backend performance comparison to run.

**Migration**: The storage benchmark is retained as a single-backend bench (`kenn-store/benches/storage_harness.rs`).
