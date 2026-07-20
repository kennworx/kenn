## Context

`kenn-store` currently exposes two concrete types — `SurrealdbSink` (writer)
and `ReadDb` (reader) — implemented entirely against `surrealdb 3.0.5` with
SurrealQL strings throughout `src/db.rs` (~1800 LOC). The crate publishes
~25 semantic read methods (`find_symbol_tiered`, `search_symbols_blended`,
`list_inbound`, `find_at_location`, etc.).

A writer abstraction already exists, but in the wrong crate:
`crates/kenn-indexer/src/sink.rs` declares `trait Sink` (with
`begin_run` / `write_batch(&RecordBatch)` / `end_run`),
`struct RecordBatch` as the value type, and a `BatchingSink<S: Sink>`
adapter that turns per-record producer pushes into batched flushes at a
configurable threshold. `SurrealdbSink` already implements `Sink`.

The `RunReport` parameter on `begin_run` / `end_run` is unused: the
SurrealdbSink ignores both (`_report: &RunReport` in db.rs:303 and 315).
It is vestigial from an earlier design.

The current dependency graph is:

```
   kenn-model  ←  kenn-indexer  ←  kenn-store  ←  kenn-cli, kenn-mcp
                  (sink + pipeline + driver +   (db + workflow +
                   workflow infrastructure)     lifecycle + layout)
```

Where `kenn-store::workflow` orchestrates `kenn-indexer::pipeline` for
both `kenn index` (CLI) and MCP startup. This means the `kenn-store →
kenn-indexer` edge has TWO reasons to exist: (a) `db.rs` uses the
`Sink` trait from `kenn-indexer::sink`, and (b) `workflow.rs` drives
the indexer pipeline.

This change physically moves the writer trait into `kenn-store::api`
and physically moves `workflow.rs` into `kenn-indexer`, flipping the
single dependency edge so it points the architecturally-correct way:

```
   kenn-model  ←  kenn-store  ←  kenn-indexer  ←  kenn-cli, kenn-mcp
                  (db + lifecycle +   (pipeline + driver +
                   layout + api +     workflow orchestration)
                   Writer/Reader)
```

`kenn-store` becomes the storage layer (defines the trait, owns the
db / lifecycle / layout / staleness logic). `kenn-indexer` becomes
the producer + orchestrator (pipeline, driver, parse, transform, plus
the now-relocated `workflow.rs`). The cycle is broken because
kenn-store has zero dependency on kenn-indexer in the new graph.

This dependency flip is a side-effect of getting the trait into the
right place; it is not the goal of this change but is its enabling
condition.

Callers across the workspace name these concrete types directly:

- `crates/kenn-indexer/*` — uses `SurrealdbSink` (and the `Sink` /
  `BatchingSink` machinery, which moves with this change).
- `crates/kenn-mcp/{indexing,state,server,lib,tools}.rs` — uses `ReadDb`
  for queries.
- `crates/kenn-cli/src/cmd_index.rs` — drives indexing.

We have evaluated three external candidates as possible second backends:

- **HelixDB** — disqualified: server-first architecture, compile-and-deploy
  query model.
- **Sekejap** — disqualified: maturity (single GitHub star, ~3 months old)
  and BM25 model (batch-built, requires `REINDEX` after bulk inserts) both
  fail constraints.
- **CozoDB** — viable; deferred. Datalog-based, mature, fits embedded
  constraint.

The committed-to second backend is a **DIY composition** of pure-Rust
crates: `tantivy` (BM25), `redb` (KV / source of truth), `hnsw_rs`
(vector index). It is not built in this change; it is the *anticipated*
shape the trait surface must accommodate so that adding it later does not
require trait revision.

A separate observation — Surreal's BM25 currently misses exact class
names (`FooBarBaz` does not return `FooBarBaz` as top-1) — motivates a
shared correctness fixture suite that any backend must answer for.

## Goals / Non-Goals

**Goals:**

- Define an async trait surface in `kenn-store::api` that captures the
  full set of operations the indexer and MCP reader need today.
- Move the existing SurrealDB code under `kenn-store::backends::surreal`
  behind those traits without changing on-disk format or visible behavior.
- Allow compile-time backend selection via cargo features
  (`surreal` default; `diy` reserved for the future second backend).
- Provide a cross-backend correctness + perf fixture/bench harness that
  runs against whichever backend is feature-enabled. Include the
  `FooBarBaz` BM25 exact-name fixture as a regression case.
- Pressure-test the trait shape against a *paper design* of the DIY
  backend so that adding it later requires zero trait edits — captured as
  per-method commentary in this design.

**Non-Goals:**

- Building the DIY backend. That is a separate change, dependent on this
  one. The trait shape is informed by it; no code lands here.
- Cross-engine atomicity. The trait does not promise that a write batch
  spanning multiple internal indices commits atomically.
- Streaming cursors that span engines. Read methods return materialized
  rows.
- Runtime backend selection. The trait surface is generic / compile-time
  only — `dyn StorageBackend` is not a goal.
- Migration of on-disk snapshots. Surreal continues to read its own
  format; backends own their disk layout.
- Public stable API for external consumers. Kenn ships as a single
  binary; the trait is internal.
- Removing the `surrealdb` dependency. It stays as the default backend.

## Decisions

### D1: One `Reader` trait, one `Writer` trait — symmetric naming

Reads and writes each get a single trait, named symmetrically:

```
api::reader.rs
  trait Reader            // covers all read operations:
                          //   symbol fetch_*    (fetch_symbol{,_by_*})
                          //   defs / def_lines  (fetch_defs, ...)
                          //   files / packages  (fetch_file_*, fetch_package)
                          //   docs              (fetch_symbol_docs_row)
                          //   graph             (list_inbound, list_outbound,
                          //                      list_module_files,
                          //                      find_at_location)
                          //   text search       (search_symbols_by_name,
                          //                      find_symbol_tiered)
                          //   hybrid search     (search_symbols_blended)
                          //   catalog           (distinct_languages,
                          //                      distinct_packages,
                          //                      count_table)

api::writer.rs
  trait Writer            // moved from kenn-indexer's `Sink`,
                          // renamed for symmetry with `Reader`:
                          //   begin_run(&RunReport)
                          //   write_batch(&WriteBatch)
                          //   end_run(&RunReport)

  struct WriteBatch       // moved from kenn-indexer's `RecordBatch`,
                          // renamed; same field layout

  struct BatchingWriter<W>// moved from kenn-indexer's `BatchingSink`,
                          // renamed; same per-row → batched-flush behavior

api::types.rs             // SymbolRow, DefRow, EdgeRow, PackageRow,
                          // BlendedSymbolRow, FoundSymbolRow,
                          // RankedSymbolRow, MatchKind, FlushStats,
                          // etc. (moved from db.rs, shapes unchanged)
```

The asymmetry between "one trait for writes, capability-split for reads"
that the original draft had was deliberate but inconsistent in vocabulary.
Single `Reader` + single `Writer` matches what kenn actually needs — every
real backend will implement all read methods anyway — and keeps naming
crisp at every call site.

If a future backend can't implement all read methods (a research spike,
say), it stubs the unimplemented ones with `Err(DbError::Unsupported)`.
That's fine for spike traffic and keeps the trait stable.

**Alternatives considered:**

- *Capability-partitioned reader* (`SymbolReader`, `GraphReader`, …): more
  flexible, but no concrete need today and the asymmetry with `Writer`
  hurt naming.
- *Query-level abstraction* (`execute(query: &str) -> Rows`): rejected
  outright — backends have incompatible query languages, and the whole
  point of the abstraction is to keep them invisible to callers.

### D2: Compile-time backend selection via cargo features

```toml
# crates/kenn-store/Cargo.toml
[features]
default = ["surreal"]
surreal = ["dep:surrealdb", "dep:tokio"]
diy     = []   # reserved; populated by the diy-backend change
```

`lib.rs` exposes async factory functions that hide the backend:

```rust
pub async fn open_writer(dir: &Path, options: WriterOptions)
    -> Result<impl api::Writer, DbError>;

pub async fn open_reader(snapshot: &Path)
    -> Result<impl api::Reader, DbError>;
```

Callers store the value as `impl Writer` / `impl Reader` (or thread a
generic where they need to name a type). They never reference a backend
module path. The factories' return types are crate-private concrete
backend types under the hood (`backends::surreal::SurrealWriter` /
`SurrealReader`), made opaque via `impl Trait`.

**Alternatives considered:**

- *`dyn` traits*: async + lifetimes + streaming results behind `dyn` is
  painful, and runtime swap is not a need.
- *Generic over backend*: same call sites would have to thread `<B>`
  parameters everywhere. Type aliases are cleaner.

### D3: Writer keeps the existing `Sink`-shaped batch flow, renamed; drops unused `RunReport` parameter

The existing `Sink`/`RecordBatch`/`BatchingSink` shape works and is
already exercised end-to-end. We move it from `kenn-indexer` to
`kenn-store::api`, rename for symmetry, drop the unused `RunReport`
parameter from `begin_run`/`end_run`, and otherwise leave it alone:

```rust
trait Writer {
    fn begin_run(&mut self) -> Result<(), DbError>;
    fn write_batch(&mut self, batch: &WriteBatch)
        -> Result<(), DbError>;
    fn end_run(&mut self) -> Result<(), DbError>;
}

pub struct WriteBatch { /* same fields as today's RecordBatch */ }

pub struct BatchingWriter<W: Writer> {
    inner: W,
    batch: WriteBatch,
    threshold: usize,
}
// pushes individual records, flushes through `Writer::write_batch`
// at the threshold; matches the old `BatchingSink<S>` behavior 1-to-1.
```

The producer pushes records one at a time into `BatchingWriter`, which
accumulates into `WriteBatch` and flushes through the backend's
`Writer::write_batch` at the configured threshold. Threshold lives in
`kenn-indexer`'s ingest config; the trait does not prescribe it.

The trait is **sync**. The Surreal impl uses
`tokio::runtime::Handle::current().block_on(...)` internally to drive
async SurrealDB calls (this is how it works today; we preserve the
behavior). Future backends with naturally sync APIs (Tantivy + redb +
hnsw_rs) implement it directly. This intentionally diverges from the
async `Reader` (D4) — the writer runs on a sync ingest pipeline driven
from blocking threads, the reader serves async MCP traffic.

**Alternatives considered:**

- *Per-row `async add_*` with associated `Batch`*: cleaner on paper,
  but requires rewriting `BatchingSink` and the indexer's ingest loop
  for no behavioral gain.
- *Async `Writer`*: forces the indexer's blocking loop to either spawn
  per call or hold a runtime. The current sync trait + internal
  `block_on` is the path of least resistance and is already proven.

### D4: `Reader` is async, `Writer` is sync — by intent, not accident

`Reader` is async because MCP serves async traffic and Surreal's
read path is async-native. The DIY backend's component crates (tantivy,
redb, hnsw_rs) are sync; their reader impl wraps calls in
`tokio::task::spawn_blocking`.

`Writer` is sync (D3) because the ingest pipeline is sync and CPU-bound,
driven from blocking threads under `tokio::task::block_in_place`. This
matches the existing `Sink` contract. Surreal's async writes are driven
from inside `Writer::write_batch` via `Handle::current().block_on(...)`
— the same pattern used today.

**Alternatives considered:**

- *Both traits async*: forces the indexer's blocking loop to either
  spawn per call or hold a runtime; preserves nothing useful.
- *Both traits sync*: forces Surreal's reader to block on the MCP
  runtime in every fetch. Bad ergonomics and unnecessary.

### D5: Hybrid search is a method, not a contract over its components

`Reader::search_symbols_blended` returns `Vec<BlendedSymbolRow>`.
Surreal's impl will use its native blended query. The DIY impl will run
the BM25 query on tantivy and the kNN query on hnsw_rs, then fuse with
RRF (or weighted) in Rust. The trait does not expose the pieces; the
backend chooses.

**Alternatives considered:**

- *Expose `text_search` and `vector_search` and let the caller fuse*:
  pushes ranking decisions out of the storage layer. Bad — those
  decisions need backend-specific tuning (analyzer choice, vector index
  ef parameter).

### D6: Trait does NOT promise cross-engine atomicity

Explicit non-goal in the trait docs and harness. Recovery posture for
backends that lack atomic multi-engine commit: rebuild from source
corpus. kenn already supports re-indexing; the harness exercises a
crash-mid-flush case where post-flush queries may show partial state,
and the documented remedy is re-ingest.

### D7: Bench / fixture harness shape

Lives at `crates/kenn-store/benches/storage_harness/` (criterion
benches) plus `tests/storage_fixtures/` (correctness tests using the
trait re-exports). Both compile against whichever backend is enabled
via the active feature.

Required fixtures (specs will state these as requirements):

```
   correctness:
     - exact name BM25: "FooBarBaz" → top-1 = FooBarBaz
     - exact pub_id lookup
     - inbound/outbound edges round-trip
     - find_at_location returns enclosing symbol
     - blended search returns at least one BM25 + vector candidate
     - distinct_languages / distinct_packages match ingest
   perf (reported, not asserted):
     - bulk ingest 10k symbols + edges throughput
     - producer→queryable lag (time from add_symbol to fetchable)
     - p50 / p95 query latency for find_symbol_tiered
     - p50 / p95 latency for search_symbols_blended
```

Surreal must pass the correctness fixtures except where pre-existing
bugs are documented (the `FooBarBaz` case is currently expected to
*fail* on Surreal; we mark it `#[ignore = "tracked: surreal bm25 miss"]`
or assert-with-known-issue so adding the DIY backend later flips it
green).

### D8: Migration sequence and dependency flip

The dependency edge between `kenn-store` and `kenn-indexer` flips. To
do this without an intermediate broken state we:

1. Move the writer abstraction into `kenn-store::api::writer`. Renamed:
   `Sink → Writer`, `RecordBatch → WriteBatch`,
   `BatchingSink → BatchingWriter`. `SinkError` folds into `DbError`
   (adds a `Serde(serde_json::Error)` variant). Drop the unused
   `RunReport` parameter from `begin_run`/`end_run`.
2. Move `crates/kenn-store/src/workflow.rs` into
   `crates/kenn-indexer/src/workflow.rs`. Internal imports adjust
   (`crate::db::SurrealdbSink` becomes `kenn_store::open_writer(...)`,
   etc.).
3. Flip Cargo.toml deps: remove `kenn-indexer` from `kenn-store`'s
   `[dependencies]`; add `kenn-store` to `kenn-indexer`'s.
4. Delete `crates/kenn-indexer/src/sink.rs` after pipeline.rs and
   transform_jsonl.rs migrate their imports.
5. `kenn-cli` and `kenn-mcp` import `index_workspace` from
   `kenn_indexer` instead of `kenn_store`. They keep importing
   storage types (`SymbolRow`, `Store`, `open_for_read`, etc.) from
   `kenn_store`. Reader/writer types come from `kenn_store::api`.

The original `kenn_indexer::sink` module — `Sink`, `RecordBatch`,
`BatchingSink`, `SinkError` — is deleted; its contents now live in
`kenn_store::api` under the new names.

No version bumps. Renames in place. Concrete types under
`backends::surreal` (`SurrealWriter`, `SurrealReader`) are
crate-private and reached only via factories.

## Risks / Trade-offs

- **[Trait shape may not fit DIY perfectly]** → Mitigation: this design
  was authored after sketching the DIY backend's shape (sync, multi-engine
  commits, RRF fusion). If a real edge surfaces during the diy-backend
  change, we accept a follow-up trait revision rather than building
  speculative flexibility now.

- **[Doubling the surface — `db.rs` types now live behind a trait]** →
  Mitigation: the trait file is the only addition; the implementation
  moves to `backends/surreal/` largely unchanged. Net LOC is ~equal.

- **[Bench harness is the most code in this change]** → Trade-off
  accepted: the harness is the deliverable that justifies the
  abstraction even if no second backend ever lands. Without it, this
  change is just a refactor.

- **[Async trait churn on `Reader`]** → We use `#[async_trait]` only
  if needed for readability; with current MSRV native async traits are
  fine. Decision made during implementation.

## Migration Plan

Single PR, behavior-preserving:

1. Add `kenn-store::api` module with `types`/`reader`/`writer`
   submodules. Move row types in (DONE in chunk A).
2. Define the `Writer` trait + move `WriteBatch` / `BatchingWriter`
   from `kenn-indexer/src/sink.rs` into `kenn-store/src/api/writer.rs`.
   Drop the unused `RunReport` parameter on `begin_run`/`end_run`.
   Fold `SinkError` into `DbError`.
3. Move `crates/kenn-store/src/workflow.rs` into
   `crates/kenn-indexer/src/workflow.rs`. Update its internal imports.
4. Cargo.toml flip: drop kenn-store's dep on kenn-indexer; add
   kenn-indexer's dep on kenn-store.
5. Delete `kenn-indexer/src/sink.rs`. Update `kenn-indexer/src/lib.rs`
   to export the relocated `index_workspace` and stop exporting
   the dead sink module.
6. Update kenn-indexer's `pipeline.rs` and `transform_jsonl.rs` to
   import `Writer`/`WriteBatch`/`BatchingWriter` from
   `kenn_store::api`.
7. Move `db.rs` content into `backends::surreal::*`. Implement
   `Writer` for `SurrealWriter` and `Reader` for `SurrealReader`.
8. Add `lib.rs` factories `open_writer` / `open_reader`. Remove the
   old `pub mod db;` declaration once everything has moved.
9. Update `kenn-cli` and `kenn-mcp` call sites: factories from
   `kenn_store`, `index_workspace` from `kenn_indexer`, no direct
   reference to `SurrealdbSink` / `ReadDb`.
10. Add fixture/bench harness under `tests/` and `benches/`.
11. Run `cargo clippy --workspace --all-targets` — zero warnings.

No rollback complexity: the change is internal restructuring, on-disk
format is unchanged, no external API is broken.

## Open Questions

- **Does the bench harness fixture set already cover what
  `symbol-search-redesign` plans to specify?** Cross-check before
  finalizing — duplication is fine, contradiction is not.
- **Do we want a feature flag combination test in CI** (`--no-default-features
  --features surreal` etc.)? Probably yes once a second backend exists;
  not in this change.
