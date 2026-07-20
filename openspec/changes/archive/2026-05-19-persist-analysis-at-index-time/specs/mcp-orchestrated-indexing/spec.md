## ADDED Requirements

### Requirement: Analysis phase in the index pipeline

The `kenn-indexer` `workflow::index_workspace` SHALL run an analysis phase after the aggregate-graph phase. The analysis phase SHALL:

1. Load the just-written aggregate graph in-process.
2. Call `kenn_analyze::compute_analysis(&graph, &opts)` to produce an `AnalysisResult`.
3. Persist the result via the new `Writer::write_analysis(&AnalysisResult)` method.
4. When `[index] write_report = true` (default), call `kenn_analyze::render_report(&graph, &result)` and write the output to `<workspace>/kenn-out/REPORT.md`.

The phase SHALL be gated by `[index] persist_analysis` (default `true`). When false, neither the analysis tables nor REPORT.md are written, regardless of `[index] write_report`.

The phase SHALL emit `ProgressEvent::PhaseStarted("analysis")` and `ProgressEvent::PhaseFinished("analysis")` so MCP's orchestrated-indexing status surface reflects it the same way it reflects the existing phases.

#### Scenario: Analysis phase runs after aggregation

- **WHEN** `kenn index` runs successfully against any workspace with `[index] persist_analysis = true`
- **THEN** the indexer MUST emit `PhaseStarted("analysis")` and `PhaseFinished("analysis")` events
- **AND** the resulting snapshot MUST contain populated `analysis_*` tables
- **AND** the analysis phase MUST occur after the aggregation phase and before the snapshot commit

#### Scenario: Analysis phase skipped when disabled

- **WHEN** `kenn.toml` contains `[index] persist_analysis = false`
- **AND** `kenn index` runs successfully
- **THEN** the indexer MUST NOT emit `PhaseStarted("analysis")`
- **AND** the snapshot's `analysis_*` tables MUST be empty (or absent)
- **AND** `kenn-out/REPORT.md` MUST NOT be created or modified by this run

### Requirement: Analysis options surfaced through `[index]`

The `kenn_analyze::AnalysisOptions` knobs that previously rode on `kenn analyze` CLI flags SHALL be configurable under `[index]`:

- `[index] analysis.top_n` (default `20`) — top-N for each god-node list.
- `[index] analysis.max_depth` (default `4`) — maximum hierarchy depth.
- `[index] analysis.min_cluster` (default `20`) — minimum community size to recurse into.

The workflow SHALL read these values when constructing `AnalysisOptions` for the analysis phase.

#### Scenario: Analysis knobs respected at index time

- **WHEN** `kenn.toml` contains `[index] analysis = { top_n = 50, max_depth = 6, min_cluster = 10 }`
- **AND** `kenn index` runs
- **THEN** the persisted `analysis_god_nodes` table MUST contain up to 50 rows per filter
- **AND** the persisted `analysis_anchored_hierarchy` tree MUST contain communities at depth 6 where the source data permits
