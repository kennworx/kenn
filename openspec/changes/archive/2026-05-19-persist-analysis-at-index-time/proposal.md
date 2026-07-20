## Why

Today `kenn analyze` recomputes the entire derived analysis — anchor map, hierarchical Louvain, flat Louvain, god-node rankings — every single time it runs (~0.2 s in release, ~3 s in debug on a 12k-node / 121k-edge workspace). None of that work is persisted anywhere, so every consumer that wants the data pays the recompute cost again. Doing the work once at index time and reading it from disk afterwards eliminates the recompute, makes the analysis available as a first-class snapshot artifact (ready for future readers including an MCP surface in a follow-up change), and clarifies the role of `kenn analyze` — which today emits both a markdown report *and* a graph visualization, two genuinely different concerns.

## What Changes

- **Index-time analysis**: at the tail end of `kenn index`, run the existing `kenn_analyze` pipeline (anchor map → hierarchical Louvain → flat Louvain → god-node ranking) and persist the results as new tables in the snapshot DB.
- **REPORT.md is written at index time** by the indexer (not by `kenn analyze` anymore) — the report is a textual rendering of the persisted analysis, and now ships with the snapshot like any other artifact.
- **BREAKING** Rename `kenn analyze` → `kenn visualize`. The renamed command is a thin reader: it loads the persisted analysis, computes the anchor layout, and writes `kenn-out/graph.html`. The `--graph [<algo>]` flag and `[analyze] graph_layout` config move with it (renamed to `[visualize]` and `--algo`). Bare `kenn visualize` writes `graph.html` with the default algorithm.
- **`[index]` config section** in `kenn.toml`:
  - `index.write_report = true` (default) — whether to materialize REPORT.md at index time.
  - `index.persist_analysis = true` (default) — whether to write the analysis tables. When false, REPORT.md is also skipped regardless of `write_report` (no point rendering from an analysis we didn't compute).
- **`kenn_analyze::analyze` is split**: the existing free function is replaced by a pair of pure builders (`compute_analysis(&graph, &opts) -> AnalysisResult`) and a renderer (`render_report(&graph, &result) -> String`). The indexer calls both at write time; `kenn visualize` calls neither — it reads the persisted shape.
- **Migration**: snapshots produced by a pre-this-change `kenn index` will lack the analysis tables. `kenn visualize` errors out with the same "run `kenn index --force`" message that currently guards the missing-aggregate case.
- **Out of scope (next proposal)**: MCP read tools that surface the persisted analysis (`list_god_nodes`, `get_communities`, `get_community_for_symbol`, anchor listing). The tables persisted by this change are the substrate that change will build on.

## Capabilities

### New Capabilities
- _(none)_

### Modified Capabilities
- `graph-analysis`: derived analysis (clusters, god-nodes) becomes a persisted snapshot artifact written at index time, not a per-invocation recompute. The `kenn analyze` command surface is renamed to `kenn visualize` (HTML-only) and the report half is owned by `kenn index`.
- `mcp-orchestrated-indexing`: index workflow now has an analysis step after aggregation, plus a report-write step, both controllable via the new `[index]` config section.
- `index-store-db`: new tables added to the snapshot schema (`analysis_god_nodes`, `analysis_flat_communities`, `analysis_anchored_hierarchy`, plus a per-symbol membership lookup).

## Impact

- **Code**:
  - `crates/kenn-analyze/src/lib.rs`: refactor `analyze()` into `compute_analysis()` + `render_report()`. The visualize command consumes both; the indexer calls them.
  - `crates/kenn-indexer/src/workflow.rs` (or a new `phase_analyze.rs`): new analysis-and-report step after aggregation. Writes the four new tables and `kenn-out/REPORT.md`.
  - `crates/kenn-store`: new `AnalysisGodNodeRow`, `AnalysisFlatCommunityRow`, `AnalysisAnchoredCommunityRow`, plus writer methods and `scan_*` readers.
  - `crates/kenn-cli`: rename `cmd_analyze.rs` → `cmd_visualize.rs`, update `main.rs` clap command, drop the report-writing path from the CLI command (it just reads + renders graph.html).
  - `crates/kenn-cli/src/starter_kenn.toml`: add `[index]` section and rename `[analyze]` → `[visualize]`.
  - `crates/kenn-config/src/lib.rs`: add `IndexConfig { write_report, persist_analysis }`, rename `AnalyzeConfig` → `VisualizeConfig`.
- **Config / behavior**: `kenn analyze` becomes `kenn visualize`; bare invocation now produces only `graph.html`, not REPORT.md (the report is written by `kenn index`). Users who scripted around `kenn analyze` need to update.
- **Storage size**: four new tables. Rough budget on the 12k-node / 121k-edge workspace — god-nodes (60 rows), flat communities (~450 rows), anchored hierarchy (~366 anchors with depth ≤ 4 each), per-symbol lookup (12k rows). ≪1 MB extra per snapshot.
- **Performance**: indexing wall time grows by ~0.15 s (release) — the flat-Louvain pass. Acceptable next to the multi-second indexing budget. `kenn visualize` drops from ~0.3 s to <0.1 s (layout + I/O only).
- **Migration**: existing snapshots can keep being read for everything that wasn't analysis. `kenn visualize` errors out with the "run `kenn index --force`" hint until the user re-indexes.
- **MCP**: out of scope; the persisted tables this change introduces are the substrate a follow-up proposal will read from.
