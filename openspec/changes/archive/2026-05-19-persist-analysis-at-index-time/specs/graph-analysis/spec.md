## ADDED Requirements

### Requirement: Analysis is a persisted snapshot artifact

The derived analysis (anchor map, hierarchical Louvain partition, flat Louvain partition, god-node rankings) SHALL be computed during `kenn index` and persisted as tables in the snapshot DB. Subsequent reads (the `kenn visualize` command and the MCP read tools) SHALL load this data via `Reader::scan_analysis_*` rather than recomputing.

The persisted set SHALL include:

- Top-N nodes by weighted degree for each of the three node filters (`live`, `test`, `external`).
- One row per flat community summarising size, total weight, anchor coverage, and the cross-anchor flag.
- One row per anchored-hierarchy community covering the recursive Louvain partition (depth ≥ 0, parent pointer when depth > 0), size, test-ratio, and the test-infra flag.
- One row per aggregate node mapping `short_id → (flat_community_id, anchored_leaf_community_id)` so per-symbol lookups are O(1).

#### Scenario: Analysis written at index time

- **WHEN** `kenn index` runs with `[index] persist_analysis = true` (the default)
- **THEN** the resulting snapshot MUST contain non-empty `analysis_god_nodes`, `analysis_flat_communities`, `analysis_anchored_hierarchy`, and `analysis_node_membership` tables

#### Scenario: Re-read instead of recompute

- **WHEN** `kenn visualize` runs against a snapshot whose analysis tables are populated
- **THEN** the command MUST NOT call `cluster::hierarchical`, `cluster::louvain_flat`, or `top_by_weighted_degree`
- **AND** it MUST load the persisted analysis via `Reader::scan_analysis_*`

### Requirement: REPORT.md written at index time

`kenn-out/REPORT.md` SHALL be rendered and written by the `kenn index` pipeline (in the analysis phase, after aggregation), not by the visualize / former-analyze command. The report's content SHALL be unchanged from the prior format (summary, three god-node sections, anchored hierarchy, flat communities). Emission SHALL be gated by `[index] write_report` (default `true`).

When `[index] write_report = false`, REPORT.md SHALL NOT be created or modified by the index run; any existing REPORT.md from a prior run SHALL be left untouched.

`kenn visualize` SHALL NOT write REPORT.md under any flag combination.

#### Scenario: Report emitted at index by default

- **WHEN** `kenn index` runs against a fresh workspace
- **THEN** `kenn-out/REPORT.md` MUST exist with the documented sections
- **AND** the file mtime MUST match the indexing run, not a later `kenn visualize` invocation

#### Scenario: Report suppression honored

- **WHEN** `kenn.toml` contains `[index] write_report = false`
- **AND** `kenn index` runs
- **THEN** `kenn-out/REPORT.md` MUST NOT be created (or overwritten) by this run

### Requirement: `kenn visualize` command surface

The CLI subcommand SHALL be named `kenn visualize`. It SHALL read the snapshot (including the persisted analysis), compute the anchor layout, and write `kenn-out/graph.html`. It SHALL accept:

- `--algo <spectral|force|stress|linlog>` — anchor layout algorithm. Bare `kenn visualize` resolves the algorithm from `[visualize] layout` in `kenn.toml`, falling back to `spectral` when unset. Explicit `--algo` overrides the config.
- `--workspace <path>` — same semantics as on other subcommands.

The command SHALL exit with an error and a non-zero code when the snapshot lacks the analysis tables, with a message referencing `kenn index --force`. The error message format SHALL match the message used by the existing missing-aggregate guard.

The command SHALL NOT recompute clustering or god-nodes. The command SHALL NOT write REPORT.md.

#### Scenario: Visualize reads persisted analysis and writes graph.html only

- **WHEN** `kenn visualize` runs against a snapshot whose analysis tables are populated
- **THEN** `kenn-out/graph.html` MUST be (re)written
- **AND** `kenn-out/REPORT.md` MUST NOT be modified

#### Scenario: Visualize errors on snapshots without analysis

- **WHEN** `kenn visualize` runs against a snapshot whose `analysis_god_nodes` table is empty or absent
- **THEN** the command MUST exit with a non-zero code
- **AND** stderr MUST reference `kenn index --force`

### Requirement: Analysis options live in `[index]` and `[visualize]`

The `kenn.toml` schema SHALL expose two sections relevant to analysis:

- `[index]` controls the index-time analysis + report writers:
  - `write_report: bool` (default `true`).
  - `persist_analysis: bool` (default `true`). When `false`, the analysis tables are not written and REPORT.md is not written regardless of `write_report`.
- `[visualize]` controls the visualize command:
  - `layout: Option<String>` (default unset). Sets the default `--algo` value used by bare `kenn visualize` invocations.

The previous `[analyze]` section SHALL no longer be recognised.

#### Scenario: Bare visualize uses config algorithm

- **WHEN** `kenn.toml` contains `[visualize] layout = "force"`
- **AND** the user runs `kenn visualize`
- **THEN** the emitted `graph.html` MUST use the `force` algorithm

## REMOVED Requirements

### Requirement: REPORT.md structure

**Reason:** the report continues to exist but is no longer the `kenn analyze` command's output — it is written at index time. The structural content (sections, test-infra threshold, default `top_n`) moves to the index-write requirement and is preserved unchanged. The existing requirement is removed because its name (`kenn analyze runs successfully`) is incorrect once the command is renamed and the report is owned by index.

**Migration:** equivalent content is restated in the new `Requirement: REPORT.md written at index time` (above), which references `kenn index` as the producer.

### Requirement: Fallback on pre-aggregate snapshots

**Reason:** the fallback path was removed in the prior change (`add-analyze-html-bird-eye-view`); the requirement should already have been retired then. We retire it now along with the analyze→visualize rename. Behavior is governed by the `Requirement: Aggregate snapshot required` already in the spec.

**Migration:** none — the path has not existed since the prior change shipped.

### Requirement: CLI surface

**Reason:** the `kenn analyze` command is renamed to `kenn visualize` and its flag set is reshaped — `--top-n`, `--max-depth`, `--min-cluster` move to `[index]` config (since analysis happens at index time and these are knobs of the compute), and the visualize command keeps only `--algo`. The old CLI surface is removed.

**Migration:** users invoking `kenn analyze` switch to `kenn visualize`. Users tuning `--top-n` / `--max-depth` / `--min-cluster` set the equivalents under `[index] analysis.top_n`, `analysis.max_depth`, `analysis.min_cluster` in `kenn.toml`, then re-index.

### Requirement: `graph.html` artifact

**Reason:** still produced, but by `kenn visualize` (not `kenn analyze`) and its emission is no longer opt-in via a flag — visualize's only output is the HTML. The behavior is restated in `Requirement: kenn visualize command surface` above.

**Migration:** `kenn analyze --graph` becomes `kenn visualize`; `kenn analyze` with no flag has no visualize equivalent (use `kenn index` to refresh REPORT.md).

### Requirement: CLI flag and config default for graph emission

**Reason:** the flag is renamed (`--graph` → `--algo`) and the config section is renamed (`[analyze]` → `[visualize]`); restated in `Requirement: Analysis options live in [index] and [visualize]` above.

**Migration:** rename the `kenn.toml` section header from `[analyze]` to `[visualize]` and the field from `graph_layout` to `layout`. The values (`"spectral" | "force" | "stress" | "linlog"`) are unchanged.
