## ADDED Requirements

### Requirement: trait surface in kenn-store::api

The `kenn-store` crate SHALL expose an `api` module containing two traits — `Reader` and `Writer` — plus the value types they operate on.

`Reader` SHALL be an async trait that covers every read operation the MCP reader currently consumes: per-row symbol / def / def-line / file / package / symbol-docs fetchers; graph traversal (`list_inbound`, `list_outbound`, `list_module_files`, `find_at_location`); text search (`search_symbols_by_name`, `find_symbol_tiered`); hybrid search (`search_symbols_blended`); and catalog queries (`distinct_languages`, `distinct_packages`, `count_table`).

`Writer` SHALL be a sync trait providing `begin_run()`, `write_batch(&WriteBatch)`, and `end_run()` — the same surface as the existing `kenn_indexer::sink::Sink`, renamed for symmetry with `Reader`, with the unused `RunReport` parameter removed from `begin_run` and `end_run` (the existing Surreal sink ignores both).

The `api` module SHALL also export:

- `WriteBatch` — value type accumulating per-table records (renamed from `kenn_indexer::sink::RecordBatch`, same field layout).
- `BatchingWriter<W: Writer>` — adapter that turns per-record producer pushes into batched flushes at a configured threshold (renamed from `kenn_indexer::sink::BatchingSink`, same behavior).
- The row and result types currently defined in `kenn-store/src/db.rs`: `SymbolRow`, `DefRow`, `DefLineRow`, `PackageRow`, `FileRow`, `SymbolDocsRow`, `EdgeRow`, `BlendedSymbolRow`, `FoundSymbolRow`, `RankedSymbolRow`, `MatchKind`, `SinkOptions` (renamed `WriterOptions`), `DbError`.

#### Scenario: indexer compiles against trait re-exports only

- **WHEN** `kenn-indexer` is built
- **THEN** no source file in `crates/kenn-indexer/src/` references `surrealdb`, `Surreal`, `SurrealdbSink`, or the old `kenn_indexer::sink::{Sink, RecordBatch, BatchingSink}` symbols
- **AND** all writer call sites use `kenn_store::api::{Writer, WriteBatch, BatchingWriter}` and concrete writers obtained from `kenn_store::open_writer`

#### Scenario: mcp reader compiles against trait re-exports only

- **WHEN** `kenn-mcp` is built
- **THEN** no source file in `crates/kenn-mcp/src/` references `surrealdb`, `Surreal`, or `ReadDb` directly
- **AND** all reader call sites obtain a reader via `kenn_store::open_reader` and call methods declared on `kenn_store::api::Reader`

### Requirement: compile-time backend selection via cargo features

The `kenn-store` crate SHALL select the active backend at compile time via mutually-exclusive cargo features.

`surreal` SHALL be the default feature. A `diy` feature SHALL be declared and reserved for the future Tantivy + redb + hnsw_rs backend (no implementation in this change).

`kenn-store` SHALL provide async factory functions that return the active backend's concrete type as `impl Writer` / `impl Reader`:

- `pub async fn open_writer(dir: &Path, options: WriterOptions) -> Result<impl Writer + ..., DbError>`
- `pub async fn open_reader(snapshot: &Path) -> Result<impl Reader, DbError>`

Callers SHALL obtain writers and readers through these factories and SHALL NOT name a backend module path.

#### Scenario: default build uses Surreal backend

- **WHEN** `cargo build -p kenn-store` is run with default features
- **THEN** the build succeeds
- **AND** `kenn_store::open_writer` and `kenn_store::open_reader` resolve to factories returning Surreal-backed types
- **AND** the `surrealdb` crate is in the dependency graph

#### Scenario: diy feature placeholder declared

- **WHEN** `cargo metadata` is inspected for `kenn-store`
- **THEN** the package features include `surreal` (default) and `diy`
- **AND** enabling `diy` without `surreal` either compiles to an empty backend stub or fails with a clear `compile_error!` directing the reader to the diy-backend change

### Requirement: writer trait keeps the existing batched-flush shape

`Writer` SHALL be a sync trait with three methods: `begin_run(&mut self) -> Result<(), DbError>`, `write_batch(&mut self, batch: &WriteBatch) -> Result<(), DbError>`, `end_run(&mut self) -> Result<(), DbError>`. The signatures match the existing `Sink` trait with the unused `RunReport` parameter removed from `begin_run` and `end_run`.

`BatchingWriter<W: Writer>` SHALL accept per-record pushes from the producer, accumulate them into an internal `WriteBatch`, and call `Writer::write_batch` when the configured threshold is reached. Its public API SHALL preserve the behavior of the existing `BatchingSink<S>`.

The threshold SHALL NOT be prescribed by the trait. `kenn-indexer` configures it.

#### Scenario: per-record pushes round-trip through BatchingWriter

- **WHEN** a caller wraps an `impl Writer` in `BatchingWriter` with threshold `N` and pushes `M` records (where `M >= N`)
- **THEN** the inner writer observes at least one `write_batch` call before push number `N + 1`
- **AND** all pushed records are visible after a final flush + `end_run`

### Requirement: trait does NOT promise cross-engine atomicity

The trait contract SHALL document that `Writer::write_batch` and `Writer::end_run` do not guarantee cross-index atomicity for backends composed of multiple internal stores.

A reader observing a snapshot after a partial-flush crash MAY see some indices populated and others empty for the same logical batch.

The trait SHALL NOT expose any cross-reader/writer transaction handle.

The documented recovery posture SHALL be re-ingest from the source corpus.

#### Scenario: trait docs state non-atomicity

- **WHEN** `cargo doc -p kenn-store` is generated
- **THEN** the `Writer::write_batch` and `Writer::end_run` doc comments state that cross-engine atomicity is not guaranteed and that recovery from partial flush is caller-driven re-ingest

### Requirement: hybrid search is encapsulated behind one trait method

`Reader::search_symbols_blended` SHALL accept a query payload including text + optional vector + tunable parameters and SHALL return a single ranked `Vec<BlendedSymbolRow>`.

`Reader` SHALL NOT expose the BM25 result list or vector kNN result list separately to the caller. Fusion (native blend, RRF, weighted, or otherwise) SHALL be the backend's choice.

#### Scenario: caller cannot observe fusion mechanism

- **WHEN** `kenn-mcp` invokes `search_symbols_blended` against the active reader
- **THEN** it receives a single ranked list of `BlendedSymbolRow`
- **AND** no public method on `Reader` returns the unfused BM25 or vector candidates

### Requirement: SurrealDB code SHALL live behind the abstraction

The existing SurrealDB-specific code in `crates/kenn-store/src/db.rs` SHALL move to `crates/kenn-store/src/backends/surreal/` and implement every trait declared in `kenn-store::api`.

The existing `kenn_indexer::sink` module — the `Sink` trait, `RecordBatch`, `BatchingSink`, and `SinkError` — SHALL be removed; its contents move to `kenn-store::api` under the new names defined above.

The existing `crates/kenn-store/src/workflow.rs` module SHALL move to `crates/kenn-indexer/src/workflow.rs`. The workspace dependency edge SHALL flip: `kenn-store` SHALL no longer depend on `kenn-indexer`, and `kenn-indexer` SHALL depend on `kenn-store`. Callers (`kenn-cli`, `kenn-mcp`) SHALL import `index_workspace` from `kenn_indexer` after the move.

The Surreal backend module SHALL be private to the `kenn-store` crate. Concrete types `SurrealWriter` and `SurrealReader` SHALL be reachable only through the `open_writer` and `open_reader` factories.

On-disk format SHALL be unchanged. Existing snapshots written by the prior `SurrealdbSink` SHALL open and read identically through the new trait surface.

#### Scenario: existing snapshot opens through trait

- **GIVEN** a snapshot directory written by `SurrealdbSink` prior to this change
- **WHEN** `kenn_store::open_reader(snapshot).await` is called with default features
- **THEN** all `Reader` trait methods return the same data they would have via the prior `ReadDb` API

#### Scenario: dependency edge flips between kenn-store and kenn-indexer

- **WHEN** `cargo metadata` is inspected for the workspace
- **THEN** `kenn-store` does NOT depend on `kenn-indexer`
- **AND** `kenn-indexer` depends on `kenn-store`

### Requirement: cross-backend correctness fixture harness

`kenn-store` SHALL provide a fixture suite, placed under `crates/kenn-store/tests/storage_fixtures/` (or a dedicated test crate), that compiles against the active backend feature and exercises the trait surface end-to-end via `open_writer` / `open_reader`.

The suite SHALL include at minimum:

- Exact pub_id round-trip via `Reader::fetch_symbol`.
- Exact name BM25 fixture: a symbol named `FooBarBaz` is ingested, followed by a BM25 query for the literal string `FooBarBaz`; the expected outcome is that the symbol is returned as the rank-1 result.
- Inbound and outbound edge round-trip via `Reader::list_inbound` and `Reader::list_outbound`.
- `find_at_location` returning the enclosing symbol for a known position.
- `Reader::search_symbols_blended` returning a non-empty list when both BM25-matching and vector-matching candidates exist.
- `Reader::distinct_languages` and `Reader::distinct_packages` matching the set ingested.

Fixtures that are known to fail on the Surreal backend (notably the exact-name `FooBarBaz` BM25 case) SHALL be marked with a documented expected-failure annotation that records the tracking note. They SHALL become unconditional pass requirements when a backend that fixes them is added.

#### Scenario: harness runs against Surreal with documented expected failures

- **WHEN** `cargo test -p kenn-store --features surreal` is run
- **THEN** every fixture either passes or is marked with a documented expected-failure annotation
- **AND** at least one such expected-failure annotation references the exact-name BM25 case for `FooBarBaz`

#### Scenario: harness scales to additional backends

- **GIVEN** a future `diy` feature that brings up the Tantivy + redb + hnsw_rs backend
- **WHEN** `cargo test -p kenn-store --no-default-features --features diy` is run
- **THEN** the same fixture suite executes against the DIY backend with no source-level changes to the fixtures themselves

### Requirement: cross-backend perf bench harness

`kenn-store` SHALL provide a criterion-based bench harness, placed under `crates/kenn-store/benches/storage_harness/`, that measures and reports — without asserting fixed thresholds — at minimum:

- Bulk ingest throughput for a representative batch (e.g. 10k symbols + edges).
- Producer→queryable lag (elapsed time from a record push returning to the symbol becoming returnable from a `Reader` call).
- Latency distribution (p50, p95) for `find_symbol_tiered`.
- Latency distribution (p50, p95) for `search_symbols_blended`.

The bench harness SHALL run against whichever backend is feature-enabled.

#### Scenario: bench harness emits numbers for the active backend

- **WHEN** `cargo bench -p kenn-store --features surreal` is run
- **THEN** the harness produces criterion reports for ingest, lag, `find_symbol_tiered`, and `search_symbols_blended`
- **AND** no bench asserts a fixed threshold; numbers are reported for comparison
