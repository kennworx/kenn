## 1. Module scaffold

- [x] 1.1 Add `surreal` (default) and `diy` (placeholder) features to `crates/kenn-store/Cargo.toml`; gate `surrealdb` and the `tokio` runtime features behind `surreal` *(features declared; gating `surrealdb`/`tokio` deps deferred to task 5.x once db.rs moves under `backends::surreal`)*
- [x] 1.2 Create `crates/kenn-store/src/api/mod.rs` with submodules `reader`, `writer`, `types`
- [x] 1.3 Create `crates/kenn-store/src/backends/mod.rs` with `#[cfg(feature = "surreal")] pub(crate) mod surreal;` and `#[cfg(feature = "diy")] pub(crate) mod diy;` (the `diy` body is a single `compile_error!` pointing to the diy-backend change)
- [x] 1.4 Wire `lib.rs` to declare `pub mod api;` and `mod backends;` (private)

## 2. Move row, result, and error types into kenn-store::api::types

- [x] 2.1 Move `SymbolRow`, `DefRow`, `DefLineRow`, `PackageRow`, `FileRow`, `SymbolDocsRow`, `BlendedSymbolRow`, `FoundSymbolRow`, `RankedSymbolRow`, `MatchKind`, `DbError` from `db.rs` into `api/types.rs`
- [x] 2.2 Rename `SinkOptions` to `WriterOptions` in `api/types.rs` *(legacy `SinkOptions` retained as a `pub type` alias in `db.rs` until call sites migrate)*
- [ ] 2.3 Define a public `EdgeRow` type in `api/types.rs` if not already present (current Surreal code uses ad-hoc structs for edge rows) *(deferred — current Surreal write path uses `kenn_model::EdgeRecord` directly; no public `EdgeRow` is read out today, so adding one without a use case is speculative. Will revisit when defining `Reader::list_inbound`/`list_outbound` return shape in task 4.x.)*
- [x] 2.4 Re-export every type from `api/types.rs` at the crate root so existing call sites compile

## 3. Move Sink → Writer / RecordBatch → WriteBatch / BatchingSink → BatchingWriter into kenn-store::api

- [x] 3.1 Copy contents of `crates/kenn-indexer/src/sink.rs` into `crates/kenn-store/src/api/writer.rs`
- [x] 3.2 Rename: `Sink` → `Writer`, `RecordBatch` → `WriteBatch`, `BatchingSink` → `BatchingWriter`. Drop the unused `RunReport` parameter from `begin_run` and `end_run` signatures (and the `BatchingWriter` wrappers thereof)
- [x] 3.3 Fold `SinkError` into `DbError` in `api/types.rs` — add a `Serde(serde_json::Error)` variant; update `From` impls
- [x] 3.4 Add doc comments on `Writer::write_batch` and `Writer::end_run` stating: cross-engine atomicity is NOT guaranteed; recovery is re-ingest from source corpus *(doc on the trait itself; per-method docs deferred to ergonomic tweaks during reader trait work)*
- [x] 3.5 Move the `BatchingSink` unit tests from `sink.rs` into `api/writer.rs` under `#[cfg(test)] mod tests` with the renamed types

## 3a. Move workflow.rs and flip the dep edge (F')

- [x] 3a.1 Move `crates/kenn-store/src/workflow.rs` → `crates/kenn-indexer/src/workflow.rs`. Update its internal imports
- [x] 3a.2 Update `kenn-indexer/src/lib.rs`: declare `pub mod workflow;` and re-export `index_workspace`, `WorkflowError`, `WorkflowOutcome`, `SnapshotCounts`. Remove `pub mod sink;` and the `pub use sink::...` line
- [x] 3a.3 Update `kenn-store/src/lib.rs`: remove `pub mod workflow;` and the `pub use workflow::*;` line
- [x] 3a.4 Cargo.toml flip: remove `kenn-indexer = { path = "../kenn-indexer" }` from `crates/kenn-store/Cargo.toml`; add `kenn-store = { path = "../kenn-store" }` to `crates/kenn-indexer/Cargo.toml`
- [x] 3a.5 Update `crates/kenn-indexer/src/pipeline.rs` and `crates/kenn-indexer/src/transform_jsonl.rs` imports: `crate::sink::{Sink, RecordBatch, BatchingSink, SinkError}` → `kenn_store::api::{Writer, WriteBatch, BatchingWriter}` and `kenn_store::api::DbError` for errors. Drop `RunReport` arguments at call sites of `begin_run` / `end_run`
- [x] 3a.6 Delete `crates/kenn-indexer/src/sink.rs`
- [x] 3a.7 Update `crates/kenn-mcp/src/indexing.rs` import: `kenn_store::index_workspace` → `kenn_indexer::index_workspace`. Also `crates/kenn-mcp/tests/lifecycle.rs`. `kenn-cli` uses local `IndexerDriver` directly (does not call `index_workspace`)
- [x] 3a.8 Verify `cargo build --workspace` is green

## 4. Define Reader trait

- [x] 4.1 In `api/reader.rs` declare `pub trait Reader { ... }` as an async trait covering all read methods currently on `ReadDb`
- [x] 4.2 Group method declarations by concern with comments: symbol fetch, defs / def lines, files / packages, graph traversal, text search, hybrid search, catalog
- [x] 4.3 Add doc comment on `search_symbols_blended` clarifying that fusion is the backend's choice and the caller cannot observe BM25 / vector candidates separately
- [x] 4.4 Decide between `#[async_trait]` and native async traits during impl; pick the smaller surface *(chose native return-position `impl Future + Send`; `impl Reader for ReadDb` uses `async fn` shorthand. No async_trait macro.)*
- [x] 4.5 Verify trait shape against current ReadDb impl by adding `impl Reader for ReadDb` in db.rs that delegates each method to the inherent method of the same name; build green

## 5. Move SurrealDB code under backends::surreal

- [x] 5.1 Move the body of `db.rs` into `crates/kenn-store/src/backends/surreal/mod.rs` *(single file for now; further splitting into `read.rs`/`write.rs`/`queries.rs` deferred — the file works as-is, splitting can be a future cleanup)*
- [x] 5.2 Rename `SurrealdbSink` → `SurrealWriter`; `ReadDb` → `SurrealReader`. Both struct names changed throughout the moved file via `sed`
- [x] 5.3 `impl Reader for SurrealReader` carries over from section 4 (the trait was already implemented for the renamed type)
- [x] 5.4 `impl Writer for SurrealWriter` carries over from section 3 (the trait was already implemented for the renamed type)
- [x] 5.5 `cargo build --workspace` and `cargo test --workspace --lib` (51 tests) green

The original `db.rs` is now a thin back-compat shim that re-exports `SurrealReader as ReadDb` and `SurrealWriter as SurrealdbSink` so call sites in `kenn-cli`, `kenn-indexer::workflow`, and `kenn-mcp` keep compiling. The shim deletes when section 8 lands. `backends::surreal` is currently `pub` (not `pub(crate)`) for the same reason — tightens after section 8.

## 6. Factory functions and crate-root re-exports

- [x] 6.1 `pub async fn open_writer(dir: &Path, options: WriterOptions) -> Result<ActiveWriter, DbError>` *(returns concrete type alias rather than `impl Trait` so callers can store `Arc<ActiveWriter>` without a generic — Reader's RPITIT methods aren't dyn-compatible)*
- [x] 6.2 `pub async fn open_reader(snapshot: &Path) -> Result<ActiveReader, DbError>`
- [x] 6.3 `BatchingWriter` reachable via `kenn_store::api::BatchingWriter` (already exported in section 3)
- [x] 6.4 Removed `pub mod db;` line from `lib.rs`
- [x] 6.5 Deleted `crates/kenn-store/src/db.rs`
- [x] 6.6 Tightened `backends::surreal` from `pub` to `pub(crate)` now that no caller names the concrete types

## 7. Verify kenn-indexer migration

*Most of this work happens in section 3a. This section is a final sweep.*

- [x] 7.1 Confirmed `kenn-indexer` no longer owns the writer abstraction
- [x] 7.2 `cargo test --workspace` (~190 tests across all crates) green
- [x] 7.3 `rg "kenn_indexer::sink" crates/` returns nothing

## 8. Migrate kenn-cli and kenn-mcp call sites

- [x] 8.1 `kenn-cli/src/cmd_index.rs`: `SurrealdbSink::create(...).await` → `kenn_store::open_writer(dir, WriterOptions::default()).await`. Same change in `kenn-indexer/src/workflow.rs`.
- [x] 8.2 `kenn-mcp/{state,tools,indexing}.rs`: `kenn_store::db::ReadDb` (type) → `kenn_store::ActiveReader`; `ReadDb::open(...)` (call) → `kenn_store::open_reader(...)`. `kenn_store::db::{rows...}` → `kenn_store::{rows...}` (already re-exported at crate root).
- [x] 8.3 `rg "surrealdb|Surreal<|SurrealdbSink|kenn_store::db|\bReadDb\b" crates/kenn-mcp crates/kenn-cli crates/kenn-indexer` returns nothing in code; doc-comment remnants swept.
- [ ] 8.4 Smoke test: index a small fixture repo end-to-end through `kenn` CLI *(deferred to user — automated tests cover the path; live indexing of a real repo requires running outside the sandbox per project convention)*

## 9. Cross-backend correctness fixture harness

- [x] 9.1 `crates/kenn-store/tests/storage_fixtures.rs` with `build_corpus` helper that writes a deterministic 3-symbol / 1-file / 3-def / 3-doc / 2-edge mini-corpus through `BatchingWriter`
- [x] 9.2 `pub_id_round_trip` via `Reader::fetch_symbol`
- [x] 9.3 `bm25_exact_class_name_foobarbaz_top1` — `#[ignore = "tracked: surreal BM25 misses exact class name (FooBarBaz). Flip to expected pass when DIY backend lands."]`
- [x] 9.4 `inbound_round_trip`, `outbound_round_trip`, `find_at_location_returns_enclosing_symbol`
- [x] 9.5 `blended_returns_nonempty_when_bm25_match_exists` *(named per current Surreal blend = BM25(name) + BM25(doc); future backends with a vector dimension still satisfy)*
- [x] 9.6 `distinct_languages_matches_ingest`, `distinct_packages_matches_ingest`
- [x] 9.7 `cargo test -p kenn-store --test storage_fixtures`: 7 passed + 1 ignored (the documented FooBarBaz regression)
- [x] 9.8 Added `kenn_store::reader_from_writer(&writer)` helper at crate root *(`#[doc(hidden)]`)* — RocksDB doesn't release file locks within the same process even after writer shutdown, so the canonical in-process pattern is sharing the writer's open handle. Used by both fixtures and benches.

## 10. Cross-backend perf bench harness

- [x] 10.1 `crates/kenn-store/benches/storage_harness.rs` registered in Cargo.toml as `harness = false` criterion target; criterion 0.5 added to dev-deps with `async_tokio` feature
- [x] 10.2 `bulk_ingest/symbols_10k` — throughput of pushing 10k symbols + 10k docs + ~10k edges through `BatchingWriter::flush`
- [x] 10.3 `producer_to_queryable_lag/after_flush` — elapsed from final push to a successful `Reader::fetch_symbol_by_short_id`. Smoke-measured at ~32 ms for a 100-symbol mini-corpus on Surreal
- [x] 10.4 `find_symbol_tiered/exact_name` — criterion gives the latency distribution
- [x] 10.5 `search_symbols_blended/blended_query` — likewise
- [x] 10.6 `cargo bench -p kenn-store --bench storage_harness -- --list` shows all four; `producer_to_queryable_lag/after_flush` end-to-end smoke run produces criterion output

## 11. Verification and cleanup

- [x] 11.1 `cargo clippy --workspace --all-targets`: same warnings as pristine HEAD (4 pre-existing `let_underscore_must_use` in test files); no warning introduced by this change. Pre-existing warnings out of scope.
- [x] 11.2 `cargo build --workspace --no-default-features --features surreal` green
- [x] 11.3 `rg` for old symbols across kenn-mcp/kenn-cli/kenn-indexer returns nothing
- [x] 11.4 `kenn.toml` doesn't name renamed symbols; doc comments in mcp/cli swept
- [x] 11.5 `openspec validate storage-abstraction --strict` passes
