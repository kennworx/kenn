## 1. Storage schema (kenn-store)

- [x] 1.1 Add `AggregateNodeRecord` and `AggregateEdgeKey` / weight value types in `kenn-model` (or `kenn-store::api::types` if more appropriate). Mirror the persisted layout documented in the index-store-db spec.
- [x] 1.2 Add `AggregateNodeRow` and `AggregateEdgeRow` row shapes to `kenn-store::api::types`. Map enum fields to `db_name()` strings to match `SymbolRow` conventions.
- [x] 1.3 Define `AGGREGATE_NODES` and `AGGREGATE_EDGES` `TableDefinition`s in `kenn-store/src/backends/db_default/schema.rs`. Add key encoders (`aggregate_edge_key(min, max, kind) -> [u8; 12]`).
- [x] 1.4 Bump `SCHEMA_VERSION` from 1 to 2 in the same file.
- [x] 1.5 Update `schema_round_trip_test.rs` to cover the new tables and verify forward-read compatibility on a synthetic version-1 snapshot (empty aggregate tables → empty result, no error).

## 2. Reader trait surface (kenn-store)

- [x] 2.1 Add `scan_aggregate_nodes` and `scan_aggregate_edges` to `api::Reader` (impl Future return shape consistent with existing methods).
- [x] 2.2 Implement both on `DefaultReader` via `tokio::task::spawn_blocking` + redb `iter()` over the new tables. Return `Ok(Vec::new())` when the table is absent (older snapshot).
- [x] 2.3 Stub both on `SurrealReader` with `Err(DbError::Backend("…not implemented in legacy backend"))`.
- [x] 2.4 Add a unit test in `kenn-store` that opens a fixture snapshot with seeded aggregate rows and round-trips them through both reader methods.

## 3. Aggregation pass (kenn-indexer)

- [x] 3.1 Create `crates/kenn-indexer/src/aggregate.rs`. Hold the roll-up (`compute_aggregate_ids`) and edge-aggregation (`aggregate_edges_by_kind`) logic ported from the `kenn-analyze` prototype, decoupled from `Reader` (operates on already-collected `Vec<SymbolRecord>` / `Vec<(EdgeRecord)>` or equivalent).
- [x] 3.2 Implement anchor resolution: package short_id when non-zero, else first path segment of the symbol's primary def file (look up via the file table), else `<unanchored>`. Intern anchor names → u32 ids.
- [x] 3.3 Implement `aggregate::write_to_sink(writer, nodes, edges)` that persists into the new redb tables in sorted order (deterministic byte layout).
- [x] 3.4 Wire the aggregation step into `pipeline::run_pipeline_with_progress` after the final `flush()` and before `end_run`. Read symbols + relevant edge kinds back from the in-flight snapshot via the writer's handle (`reader_from_writer`).
- [x] 3.5 Add `BENCH end_run: aggregate=<ms>` to the pipeline's bench output (gated on `KENN_BENCH`).
- [x] 3.6 Add a `ProgressEvent::AggregateComputed { nodes, edges, elapsed_ms }` variant and emit it before `EndRunComplete`.

## 4. Aggregation tests (kenn-indexer)

- [x] 4.1 Unit tests for `compute_aggregate_ids` covering: method rolls up to class, free function rolls up to module, field rolls up to class, orphan stays self, cycle terminates safely.
- [x] 4.2 Unit tests for `aggregate_edges_by_kind` covering: weighted accumulation across multiple per-symbol edges, multi-kind between same pair stays as separate rows, self-loop drop, skipped kinds dropped, undirected dedup.
- [x] 4.3 Unit tests for anchor resolution covering: package wins over path fallback, path fallback returns first segment, missing both yields `<unanchored>`.
- [x] 4.4 Integration test that runs the existing in-process pipeline test fixture, calls `scan_aggregate_*` against the published snapshot, and asserts at least one node + one edge appear.
- [x] 4.5 Determinism test: ingest the same fixture twice, assert byte-identical `aggregate_nodes` + `aggregate_edges` table contents.
- [x] 4.6 Partial-ingest test: simulate one failing project and confirm the published snapshot's aggregate tables contain rolled-up data from successful projects.

## 5. Hierarchical clustering (kenn-analyze)

- [x] 5.1 Replace `cluster::louvain(&Projection) -> Vec<Vec<ShortId>>` with `cluster::louvain_flat(&Projection) -> Partition` (rename only; preserves prototype behavior).
- [x] 5.2 Add `Hierarchy` type representing the tree: root → per-anchor branches → recursive Louvain children.
- [x] 5.3 Add `cluster::hierarchical(&Projection, &AnchorMap, opts: HierarchyOptions) -> Hierarchy`. Anchor partition at L0, single-level Louvain on each induced subgraph for L1, recurse with the same Louvain until depth or min-size halts.
- [x] 5.4 Make all iteration sorted (anchor name asc; nodes by short_id; community size desc) so the result is deterministic.
- [x] 5.5 Unit tests for `hierarchical` covering: small graph with two anchors, depth/min-cluster halting, determinism across two calls.

## 6. Projection module split (kenn-analyze)

- [x] 6.1 Extract a `projection::AggregatedGraph` type from the current `Projection` so that the same downstream code can be fed by either (a) freshly computed projection (fallback) or (b) loaded-from-snapshot aggregate tables.
- [x] 6.2 Add `projection::load_from_reader<R: Reader>(reader: &R) -> Result<AggregatedGraph, DbError>` that uses `scan_aggregate_*` and constructs the graph in memory.
- [x] 6.3 Keep `projection::build` (the existing prototype path) as the fallback used when `load_from_reader` returns an empty graph.
- [x] 6.4 Add an `AnchorMap` view derived from `AggregatedGraph` (anchor_id → set of aggregate ids).

## 7. Report rendering (kenn-analyze)

- [x] 7.1 Update `report::render` signature to accept `&Hierarchy` and the flat `Partition` together (drop the legacy flat-only signature).
- [x] 7.2 Render the anchored hierarchy section: one heading per anchor, three buckets (live / mixed / test infra) per anchor, nested levels per the design's pseudo-render shape.
- [x] 7.3 Render the flat-cross-check section: list flat communities with their member anchors and a cross-anchor flag when there are more than one.
- [x] 7.4 Preserve the existing three god-node sections (live / test / external) and per-community test-ratio tagging at the 60% threshold.

## 8. CLI plumbing (kenn-cli)

- [x] 8.1 Add `--max-depth` (default 4) and `--min-cluster` (default 20) flags to the `Analyze` subcommand alongside the existing `--top-n`.
- [x] 8.2 Wire the new flags into `kenn_analyze::AnalyzeOptions`.
- [x] 8.3 Update `cmd_analyze::run` to call the new analyze entry point (load-from-snapshot path with prototype fallback).
- [x] 8.4 Emit the single-line "snapshot pre-dates aggregate tables; consider `kenn index --force`" warning when the fallback path is taken.

## 9. Documentation

- [x] 9.1 Update the project root `README.md` (or wherever `kenn analyze` is currently documented) to describe the new report sections and CLI flags.
- [x] 9.2 Document the aggregate tables in `crates/kenn-store/src/backends/db_default/schema.rs` module docs.
- [x] 9.3 Document `compute_aggregate_ids` and `aggregate_edges_by_kind` in `crates/kenn-indexer/src/aggregate.rs` module docs (focus on the WHY of the roll-up rules and weight choices).

## 10. End-to-end validation

- [x] 10.1 Run `kenn index` then `kenn analyze` against a multi-crate Rust workspace; eyeball that the anchored hierarchy's top-level matches the crate boundaries.
- [x] 10.2 Same against a representative C# enterprise repo; verify that `pkg`-anchored top level produces meaningful subsystem groupings and that the flat cross-check surfaces at least one cross-anchor community of interest.
- [x] 10.3 Same against a TypeScript monorepo; verify path-prefix anchor fallback produces sensible top-level groupings.
- [x] 10.4 Verify `KENN_BENCH=1 kenn index` reports `aggregate=<ms>` < 10% of `run_pipeline_total` on each workspace.
- [x] 10.5 Re-run `kenn analyze` on a snapshot indexed by a kenn binary built before this change; verify the fallback warning fires and the report renders correctly.
- [x] 10.6 `cargo clippy --workspace --all-targets` clean; `cargo test --workspace` passes.
