## MODIFIED Requirements

### Requirement: trait surface in kenn-store::api

The `kenn-store` crate SHALL expose an `api` module containing the `Reader` trait plus the value types storage operations work with. The ingestion-run lifecycle (`begin` / `write_batch` / `end`) SHALL NOT be a `kenn-store` trait — it is owned by `kenn-indexer` (see the `indexing-orchestrator` capability).

`Reader` SHALL be an async trait that covers every read operation the MCP reader consumes: per-row symbol / def / def-line / file / package / symbol-docs fetchers; graph traversal (`list_inbound`, `list_outbound`, `list_module_files`, `find_at_location`); text search (`search_symbols_by_name`, `find_symbol_tiered`); hybrid search (`search_symbols_blended`); and catalog queries (`distinct_languages`, `distinct_packages`, `count_table`).

`kenn-store` has a **single storage backend** built on **one storage engine, Lance**. Every database it persists — the code graph and the search / knowledge store — is a Lance dataset; no other storage engine is used. It SHALL expose that backend's ingestion, aggregate, and finalize operations as public inherent methods on its concrete writer type, and SHALL NOT wrap those operations in an ingestion-lifecycle trait. The concrete reader and writer types SHALL be reachable as the crate-root `DbReader` / `DbWriter` types returned by the `open_reader` / `open_writer` factory functions; callers SHALL obtain readers and writers through those factories and SHALL NOT name a backend module path.

The `api` module SHALL also export:

- `WriteBatch` — value type accumulating per-table records, consumed by the backend's `write_batch` operation.
- The row and result types: `SymbolRow`, `DefRow`, `DefLineRow`, `PackageRow`, `FileRow`, `SymbolDocsRow`, `BlendedSymbolRow`, `FoundSymbolRow`, `RankedSymbolRow`, `MatchKind`, `WriterOptions`, `DbError`.

The `BatchingWriter` adapter SHALL be removed; each language ingester batches records inline and appends them to the store through its own writer (see the `indexing-orchestrator` capability).

#### Scenario: indexer drives the backend through inherent operations

- **WHEN** `kenn-indexer` is built
- **THEN** each language ingester calls the backend's public inherent operations and `kenn_store::api::WriteBatch`
- **AND** no source file references a `kenn_store::api::Writer` ingestion-lifecycle trait or `kenn_store::api::BatchingWriter`

#### Scenario: mcp reader compiles against the Reader trait only

- **WHEN** `kenn-mcp` is built
- **THEN** all reader call sites obtain a reader via `kenn_store::open_reader` and call methods declared on `kenn_store::api::Reader`

#### Scenario: kenn-store exposes no ingestion-lifecycle trait

- **WHEN** `kenn-store` is built
- **THEN** its `api` module declares `Reader` but no `Writer` ingestion-lifecycle trait
- **AND** the backend's ingestion operations are reachable as public inherent methods on `DbWriter`

#### Scenario: the backend depends on no engine but Lance

- **WHEN** `kenn-store` is built
- **THEN** neither `redb` nor `bincode` appears in the `kenn-store` dependency tree
- **AND** the code graph and the search / knowledge store are both Lance datasets

## REMOVED Requirements

### Requirement: trait does NOT promise cross-engine atomicity

**Reason**: The hazard this requirement described — a write landing in one internal store but not another, leaving a reader to observe a partially-populated snapshot — no longer exists. The backend is now a single storage engine (Lance), and every Lance dataset a run produces is built in one `building/` directory and made live by a single atomic directory swap (see `index-store-db`, "Aggregate tables written during end_run", and the `indexing-orchestrator` snapshot lifecycle). A reader only ever observes a fully-built, published snapshot; there is no partial-flush window to warn about.

**Migration**: Recovery posture is unchanged — a run that fails before the publish swap leaves `building/` unpublished and the prior `live` snapshot intact; recovery is to re-run `kenn index`, which discards the stale `building/` directory. Callers that previously relied on the documented non-atomicity caveat need no change: the guarantee is now stronger (whole-snapshot atomicity), not weaker.
