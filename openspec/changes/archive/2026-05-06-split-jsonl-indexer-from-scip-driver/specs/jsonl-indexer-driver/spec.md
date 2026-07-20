## ADDED Requirements

### Requirement: One process invocation per workspace

A `JsonlIndexer` SHALL be invoked at most once per `kenn index` run per
workspace. The pipeline MUST NOT split a workspace across multiple
invocations of the same indexer; project-list partitioning and
scheduling decisions belong to the indexer itself.

The indexer MAY internally choose any execution strategy across the
projects it discovers (sequential, parallel, batched), provided the
emitted JSONL stream conforms to the wire-format spec.

#### Scenario: Multi-solution workspace

- **WHEN** a workspace contains multiple `.sln` files configured in
  `kenn.toml` (e.g. `App.sln`, `Worker/Worker.sln`,
  `Worker/Common/Common.sln`)
- **THEN** the pipeline MUST invoke the JSONL indexer exactly once,
  passing the full project list to that single invocation
- **AND** MUST NOT spawn one indexer process per `.sln`

#### Scenario: Empty configured project list

- **WHEN** `kenn.toml` does not configure `[language.*].projects` for a
  JSONL indexer
- **THEN** the pipeline MUST still invoke the indexer once, passing the
  workspace path
- **AND** the indexer is responsible for discovering its own units
  inside that workspace

### Requirement: Indexer owns project discovery

A `JsonlIndexer` SHALL receive only the workspace path and any
indexer-specific configuration (e.g. `projects` list from `kenn.toml`)
on invocation. The pipeline MUST NOT pre-discover units on the
indexer's behalf.

When the indexer-specific `projects` list is empty, the indexer SHALL
discover units by scanning the workspace using its own rules. The
discovery rule MUST match the previous Rust-side behaviour for the
same indexer to avoid silent regressions: prefer `.sln` files over
`.csproj` when both are present, and exclude conventional build
directories (`bin/`, `obj/`, `target/`).

#### Scenario: Discovery parity with explicit project list

- **GIVEN** a workspace with one or more `.sln` files
- **WHEN** the indexer is invoked once with `kenn.toml` listing those
  same `.sln` files explicitly
- **AND** when the indexer is invoked once with no configured
  `projects` (forcing internal discovery)
- **THEN** the JSONL output MUST cover the same set of `.sln` files
  in both cases (counts match for files / symbols / edges)

### Requirement: Streaming JSONL outcome

`JsonlIndexer::run` SHALL return a structured outcome that exposes:
- the spawned process handle, for lifecycle management,
- the child's stdout pipe, streaming the JSONL frames,
- an optional captured stderr handle, for diagnostic surfacing,
- a `RunReport` covering the invocation as a whole.

The pipeline ingests stdout frame-by-frame as they arrive. The pipeline
MUST NOT wait for the indexer process to exit before beginning ingest.

#### Scenario: Streaming ingestion

- **WHEN** a JSONL indexer emits frames for the first `.sln` while the
  walk for subsequent `.sln`s is still in progress
- **THEN** the pipeline MUST consume and ingest those frames into the
  sink immediately
- **AND** MUST NOT buffer the entire stream until process exit

### Requirement: One run report per invocation

For a JSONL indexer invocation covering N `.sln`s, the pipeline SHALL
produce exactly one `RunReport`. Per-`.sln` failure attribution
SHALL be preserved by populating the report's `failed_projects` from
the `path` field of any `ErrorFrame{severity: error}` frames in the
stream.

The report's unit identifier MAY be a synthetic value such as the
workspace root path or the indexer language id; it MUST NOT be a
single `.sln` path because the invocation covers multiple.

#### Scenario: Per-sln msbuild failures still surface

- **GIVEN** a workspace where two of three `.sln`s have msbuild
  failures
- **WHEN** the indexer emits `ErrorFrame{severity:"error",
  source:"msbuild", path:"<sln1>"}` and another for `<sln3>`
- **THEN** the resulting RunReport's `failed_projects` MUST list
  both `<sln1>` and `<sln3>`
- **AND** `kenn status` MUST show those two `.sln`s as failed

### Requirement: SCIP driver contract is unchanged

The pipeline SHALL preserve the existing per-unit invocation contract
for SCIP-producing indexers. The `ScipDriver` trait (renamed from the
prior generic `LanguageDriver`) MUST retain its
`discover_units(ws) -> Vec<Unit>` step followed by `run_unit(unit, ws)
-> ScipOutcome` per unit.

The pipeline MUST continue to call `discover_units` once per SCIP
driver and loop `run_unit` over the returned units. SCIP indexers
expect this shape (rust-analyzer indexes one crate at a time;
scip-typescript indexes one tsconfig at a time) and the change to the
JSONL trait MUST NOT alter their behaviour.

#### Scenario: SCIP driver unit loop preserved

- **WHEN** a SCIP driver returns N units from `discover_units`
- **THEN** the pipeline MUST call `run_unit` exactly N times, once
  per unit
- **AND** MUST produce one RunReport per unit, as it does today
