# jsonl-indexer-driver

## Purpose

Defines the contract by which the kenn-cli pipeline invokes streaming-JSONL indexers (today: kenn-dotnet). One process is invoked per workspace, and the indexer owns project discovery and scheduling — the pipeline does not partition the workspace across multiple indexer invocations. SCIP-producing drivers retain their separate per-unit contract under the `ScipDriver` trait.
## Requirements
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
produce exactly one `RunReport`. Per-project failure attribution
SHALL be preserved by populating the report's `failed_projects` from
`ErrorFrame{severity: error}` frames in the stream, formatted from the
frame's `source`, `path` (when present), and `message`. The number of
retained entries SHALL be bounded (first 32); attributions past the cap
SHALL be recorded as a structured `failed_overflow` count on the report —
never as a synthetic list entry — so counting consumers see the true
total (`failed_projects.len() + failed_overflow`) and display surfaces
render the overflow as a `+N more` suffix.

Severity SHALL be validated at parse time, case-insensitively: `error`
(and any unrecognized severity — fail loud, never silently drop
attribution) degrades the report; `warning` does not. A stream containing
at least one such error frame SHALL degrade the report's status to
`Partial` unless the report is already `Failed`.

Warning-severity frames SHALL be recorded on the report's `warnings`
(bounded like `failed_projects`, with a structured `warnings_overflow`),
status-neutral — producers emit them for degradations that keep the run
useful (e.g. stale index-store units kept on a trusted read), and a
warning that dies in a counter silences a diagnostic the producer
promised the user.

When the indexer process exits non-zero, the failure message recorded in
`failed_projects` SHALL name the report's producer (`indexer_name`, e.g.
`kenn-ts`) — stable even under runner-form command configs such as
`["dotnet", "kenn-dotnet.dll"]`.

The report's unit identifier MAY be a synthetic value such as the
workspace root path or the indexer language id; it MUST NOT be a
single `.sln` path because the invocation covers multiple.

#### Scenario: Per-project load failures surface with paths

- **GIVEN** a workspace where two of three `.sln`s fail to load
- **WHEN** the indexer emits `ErrorFrame{severity:"error",
  source:"indexer", path:"<sln1>"}` and another for `<sln3>` (per-entry
  load failures carry the entry path; msbuild workspace diagnostics are
  message-only and surface as pathless `msbuild: <message>` attributions)
- **THEN** the resulting RunReport's `failed_projects` MUST list
  both `<sln1>` and `<sln3>`
- **AND** the report status MUST be `Partial`
- **AND** `kenn status` MUST show those two `.sln`s as failed

#### Scenario: error frames degrade a clean exit to Partial

- **WHEN** a JSONL indexer emits one `ErrorFrame{severity:"error"}` and
  then exits 0
- **THEN** the unit's report status MUST be `Partial`, not `Success`

#### Scenario: warnings do not degrade status but are surfaced

- **WHEN** a JSONL indexer emits only `ErrorFrame{severity:"warning"}`
  frames and exits 0
- **THEN** the unit's report status MUST remain `Success`
- **AND** the warnings MUST be recorded on the report and shown by
  `kenn status`

#### Scenario: overflow is a count, not a list entry

- **WHEN** a stream carries 40 `severity:"error"` frames
- **THEN** the report retains 32 attributions and `failed_overflow` = 8
- **AND** no `failed_projects` entry is a synthetic marker — `kenn status`
  reports the true count (40) and renders `+8 more` at display time only

#### Scenario: a non-zero exit names the right producer

- **WHEN** `kenn-ts` exits non-zero
- **THEN** the `failed_projects` message MUST contain `kenn-ts`, not the
  name of a different producer, even when the configured command is a
  runner form

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

### Requirement: Multiple JSONL producers with isolated id partitions

The pipeline SHALL support more than one registered `JsonlIndexer` (today: `kenn-dotnet` for C# and `kenn-ts` for TypeScript), invoking each at most once per workspace per run. Each JSONL producer's stream SHALL be ingested under its own `IdRegistry` (its own language partition), so producer-assigned `Ref`s from different producers never collide. The per-workspace single-invocation and indexer-owns-discovery contracts apply to every registered producer independently.

#### Scenario: C# and TypeScript producers coexist

- **WHEN** a workspace has both C# (`.sln`/`.csproj`) and TypeScript (`tsconfig.json`) sources and both languages are enabled
- **THEN** the pipeline invokes `kenn-dotnet` once and `kenn-ts` once
- **AND** each stream is ingested in its own id partition, with no `Ref` collision between the two

#### Scenario: TypeScript producer registered in place of the SCIP driver

- **WHEN** the runner is configured for TypeScript
- **THEN** `kenn-ts` is registered as a `JsonlIndexer` and no `scip-typescript` `ScipDriver` is registered

### Requirement: JSONL producers may be implemented in any language

A `JsonlIndexer` SHALL be free to be implemented in any host language and distributed as any executable form, provided it conforms to the JSONL wire and the invocation contract. `kenn-dotnet` is a self-contained .NET single-file binary; `kenn-ts` is a `bun build --compile` single-file executable embedding the TypeScript compiler. The pipeline treats both uniformly as spawned processes streaming frames on stdout.

#### Scenario: Compiled TypeScript producer is spawned like the C# producer

- **WHEN** the pipeline runs the TypeScript producer
- **THEN** it spawns the `build/kenn-ts` executable and ingests its stdout JSONL frame-by-frame, identically to how it spawns `build/kenn-dotnet`

