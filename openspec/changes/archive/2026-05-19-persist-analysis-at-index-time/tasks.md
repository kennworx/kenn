## 1. Refactor `kenn-analyze::analyze` into pure builders

- [x] 1.1 Introduce `pub struct AnalysisResult { anchors, hierarchy, flat, god_live, god_test, god_external }` in `crates/kenn-analyze/src/lib.rs`. Move the existing intermediate types (`HierarchyTree`, `FlatCommunity`, `RankedNode`) so they live alongside it.
- [x] 1.2 Introduce `pub struct AnalysisOptions { top_n, max_depth, min_cluster }`; remove these fields from `AnalyzeOptions` (which becomes visualize-only).
- [x] 1.3 Add `pub fn compute_analysis(graph: &AggregatedGraph, opts: &AnalysisOptions) -> AnalysisResult` — pure, no IO.
- [x] 1.4 Add `pub fn render_report(graph: &AggregatedGraph, result: &AnalysisResult) -> String`.
- [x] 1.5 Delete the existing `pub async fn analyze(reader, out_dir, opts)`. Callers move to either the index pipeline (which uses `compute_analysis` + `render_report`) or `kenn visualize` (which uses neither — see §3).
- [x] 1.6 Unit tests: `compute_analysis` is deterministic, `render_report` produces the documented section headers.

## 2. Storage schema for persisted analysis (kenn-store)

- [x] 2.1 Add `GodNodeFilter { Live, Test, External }` enum (with `db_name()` matching the convention used by `EdgeKind`).
- [x] 2.2 Define new row + record types in `crates/kenn-store/src/api/types.rs`:
  - `AnalysisGodNodeRow / Record { filter, rank, short_id, weighted_degree, name, kind, anchor_id, anchor_name }`
  - `AnalysisFlatCommunityRow / Record { community_id, size, total_weight, cross_anchor, primary_anchor_id, primary_anchor_name }`
  - `AnalysisAnchoredCommunityRow / Record { community_id, parent_id: Option<u32>, depth, anchor_id, size, test_ratio, test_infra }`
  - `AnalysisNodeMembershipRow / Record { short_id, flat_community_id, anchored_leaf_community_id }`
- [x] 2.3 Surrealdb backend: add table definitions and serializers behind `db_surreal`.
- [x] 2.4 Default (sled-like) backend: add table definitions and serializers behind `db_default`.
- [x] 2.5 Extend `Writer` trait with `fn write_analysis(&mut self, result: &AnalysisWriteSet) -> Result<()>` where `AnalysisWriteSet` is the in-memory shape that maps 1:1 to the four tables. The write SHALL run inside the current snapshot transaction.
- [x] 2.6 Extend `Reader` trait with:
  - `async fn scan_analysis_god_nodes(filter: GodNodeFilter) -> Vec<AnalysisGodNodeRow>`
  - `async fn scan_analysis_flat_communities() -> Vec<AnalysisFlatCommunityRow>`
  - `async fn scan_analysis_anchored_hierarchy() -> Vec<AnalysisAnchoredCommunityRow>`
  - `async fn scan_analysis_node_membership() -> Vec<AnalysisNodeMembershipRow>`
- [x] 2.7 All four `scan_*` methods MUST return `Ok(vec![])` (not error) on snapshots that lack the tables.
- [x] 2.8 Storage tests: write → read round-trip; replace-on-reindex (no leftover rows); empty-table read on a fresh writer.

## 3. Indexer pipeline integration (kenn-indexer)

- [x] 3.1 Add `IndexConfig { write_report: bool = true, persist_analysis: bool = true, analysis: AnalysisOptions }` to `kenn-config`.
- [x] 3.2 In `crates/kenn-indexer/src/workflow.rs` (or a new `phase_analyze.rs`), add a phase that runs after aggregation and before snapshot commit. The phase:
  - Calls `projection::load_from_reader(&self.reader)` to get the aggregate graph (or reuses the in-process value from the aggregation phase if available).
  - Calls `compute_analysis(&graph, &opts.analysis)`.
  - Maps the result into the `AnalysisWriteSet` shape (deriving `node_membership` from the hierarchy + flat partitions).
  - Calls `Writer::write_analysis(&set)`.
  - If `[index] write_report` is true: renders the report and writes `kenn-out/REPORT.md`.
- [x] 3.3 Gate the entire phase on `[index] persist_analysis`; emit `PhaseStarted("analysis")` / `PhaseFinished("analysis")` events when active.
- [x] 3.4 When `persist_analysis = false`, ensure REPORT.md is also skipped regardless of `write_report` (document the precedence in the config comment).
- [x] 3.5 Indexer integration test: a fresh index run produces a snapshot whose `Reader::scan_analysis_*` all return non-empty results and whose `kenn-out/REPORT.md` matches the legacy report format.

## 4. CLI rename: `analyze` → `visualize` (kenn-cli)

- [x] 4.1 Rename `crates/kenn-cli/src/cmd_analyze.rs` → `cmd_visualize.rs`. Drop the parts that load Louvain / god-nodes; the command becomes: open reader → scan analysis tables (only as a sanity check) → `layout::compute` → `graph::render` → write `kenn-out/graph.html`.
- [x] 4.2 `crates/kenn-cli/src/main.rs`: rename the `Analyze` clap command to `Visualize`; replace `--graph [<algo>]` with `--algo <algo>`. Update the `--workspace`, `--config` plumbing the same way as before.
- [x] 4.3 Remove the `--top-n` / `--max-depth` / `--min-cluster` flags from the CLI (they're now `[index] analysis.*` config).
- [x] 4.4 If the snapshot lacks analysis tables, exit with the documented `kenn index --force` error message.
- [x] 4.5 Status line on stdout names only the HTML file (no more "+ REPORT.md").
- [x] 4.6 `cmd_init`'s starter `kenn.toml`: add `[index]` section with `write_report`, `persist_analysis`, `analysis.{top_n,max_depth,min_cluster}` keys (commented at defaults); rename `[analyze]` → `[visualize]` with `layout` instead of `graph_layout`.
- [x] 4.7 Update `crates/kenn-cli/src/starter_kenn.toml` to reflect the new section names.

## 5. Config crate (kenn-config)

- [x] 5.1 Rename `AnalyzeConfig { graph_layout }` → `VisualizeConfig { layout }`.
- [x] 5.2 Add `IndexConfig { write_report, persist_analysis, analysis: AnalysisOptions }`. Default impls: both bools default true; `analysis` defaults to `{ top_n: 20, max_depth: 4, min_cluster: 20 }`.
- [x] 5.3 Wire the new sections into the top-level `Config` (`[index]` and `[visualize]`).
- [x] 5.4 Config tests: empty `kenn.toml` produces the documented defaults; explicit `[index] persist_analysis = false` round-trips.

## 6. Migration + breakage handling

- [x] 6.1 `kenn visualize` against a pre-this-change snapshot exits with a non-zero code and prints the `kenn index --force` hint (no panic, no fallback compute).
- [x] 6.2 Update `crates/kenn-cli/README` (or main `README.md` if there's no per-crate one) — replace any `kenn analyze` references with `kenn visualize` and document the new `[index]` / `[visualize]` config sections.

## 7. Tests + clippy

- [x] 7.1 `cargo test --workspace` passes.
- [x] 7.2 `cargo clippy --workspace --all-targets` clean on new code.
- [x] 7.3 No regression in existing fallback test (`crates/kenn-analyze/tests/fallback.rs`): the bail message is unchanged; the test continues to pass.

## 8. End-to-end validation

- [x] 8.1 Fresh `kenn index --force` on a small TypeScript monorepo produces both REPORT.md and populated analysis tables; `kenn visualize` writes graph.html without recomputing analysis.
- [x] 8.2 Same on the multi-crate Rust workspace.
- [x] 8.3 Same on a representative C# enterprise repo (12k nodes / 121k edges) — measure that indexing wall time grew by ≤ 250 ms (release) and that `kenn visualize` cold-time stays < 200 ms.
- [x] 8.4 Migration smoke: run the new binary against a snapshot built with the prior binary; confirm the CLI error path surfaces the `kenn index --force` hint.
