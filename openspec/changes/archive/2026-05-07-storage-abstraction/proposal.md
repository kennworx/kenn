## Why

`kenn-store` today is hard-wired to SurrealDB: the indexer's `SurrealdbSink`
and the reader's `ReadDb` both name the engine in their types and embed
SurrealQL strings throughout `db.rs`. We want to evaluate alternative
embedded backends (next candidate: pure-Rust Tantivy + redb + hnsw_rs) on
the same workload — search/ranking quality (Surreal's BM25 currently misses
exact class names like `FooBarBaz`), ingest throughput, and producer→queryable
lag. Without an abstraction we cannot bench alternatives against the
production reader/writer paths or run the same fixtures across engines.

## What Changes

- Introduce trait-level read and write contracts in `kenn-store::api` that
  the indexer and MCP reader consume instead of `SurrealdbSink` / `ReadDb`
  directly.
- Move the existing SurrealDB code under `kenn-store::backends::surreal`
  behind those traits, behavior-preserving.
- Select the active backend at compile time via a cargo feature
  (`surreal` default; future `diy` reserved for the upcoming Tantivy + redb
  + hnsw_rs backend, not added in this change).
- Re-export the active backend's concrete types as `kenn_store::Db` /
  `kenn_store::Sink` (or equivalent) so callers never name a backend.
- Add a cross-backend bench/fixture harness under `kenn-store/benches` (or
  `tests/`): correctness fixtures (including the failing `FooBarBaz`
  exact-name BM25 case) and ingest/query perf measurements that any
  backend feature flag must satisfy or report on.
- The trait contract makes explicit non-goals: no cross-engine atomicity,
  no streaming cursors that span engines, no shared transaction across
  reader and writer. Hybrid scoring (`search_symbols_blended`) is an
  internal trait method whose implementation is the backend's choice
  (native blend on Surreal, RRF in code on the future DIY backend).
- Writer contract uses a backend-owned `Batch` associated type with
  per-row `add_*` methods and a `flush` that commits. Batch size is the
  caller's choice (`kenn-indexer` config); the trait does not prescribe it.
- Async trait surface; sync backends adapt at the impl edge later
  (no sync backend lands in this change).

No version bumps. Renames in place. Existing on-disk snapshots stay
readable by the Surreal backend; no migration.

## Capabilities

### New Capabilities

- `storage-backend-abstraction`: Defines the trait surface that
  `kenn-store` exposes for read, write, search, and graph operations,
  the compile-time backend selection mechanism, and the cross-backend
  fixture/bench harness contract.

### Modified Capabilities

(none — `index-store-db` continues to describe the SurrealDB snapshot
schema unchanged; this change adds a layer above it without altering
schema semantics)

## Impact

- **Affected crates**:
  - `crates/kenn-store` — significant restructuring: new `api/` module,
    `backends/surreal/` module containing the moved `db.rs` content,
    feature flags in `Cargo.toml`, `lib.rs` re-exports.
  - `crates/kenn-indexer` — call sites swap from concrete
    `SurrealdbSink` to the trait re-export; semantic operations
    unchanged.
  - `crates/kenn-mcp` — reader call sites swap from concrete `ReadDb`
    to the trait re-export.
- **No public-API impact** outside the workspace; kenn is single-binary,
  not a library consumer surface.
- **Build/CI**: default features keep SurrealDB; bench/fixture suite
  becomes runnable per-backend (`--features surreal` for now).
- **Risk**: the trait shape is informed by an *anticipated* second
  backend (Tantivy + redb + hnsw_rs); if that anticipation is wrong, the
  trait may need adjustment when the DIY backend lands. Mitigation: the
  trait deliberately admits sync, multi-engine, code-fused-hybrid impls,
  and ships with non-goals that match what DIY can promise.
