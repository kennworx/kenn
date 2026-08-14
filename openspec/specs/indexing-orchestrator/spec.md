# indexing-orchestrator

## Purpose

Defines how `kenn-indexer` orchestrates an index run as four named, ordered
phases — prepare, ingest, aggregate, finalize — and the lifecycle that
connects per-language ingesters to a single DB-writer. The orchestrator owns
phase sequencing, run preparation and backend construction, the bounded
record channel that streams built records to the writer, the `Begin` / `End`
markers that distinguish a clean finish from a crash, the rolled-up aggregate
graph, and the atomic publish that makes a run's data visible only at the end.
## Requirements
### Requirement: Indexing runs as four ordered phases

`kenn-indexer` SHALL run an index as four named, ordered phases: **prepare**,
**ingest**, **aggregate**, **finalize**. The orchestrator SHALL own and
sequence the phases; a phase SHALL NOT begin until the previous phase has
completed.

The orchestrator SHALL drive phases 1 (prepare), 3 (aggregate), and 4
(finalize) directly, and SHALL delegate phase 2 (ingest) to ingesters.

#### Scenario: phases run in fixed order

- **WHEN** an index run executes
- **THEN** prepare completes before any ingester runs
- **AND** every ingester has completed before the aggregate phase starts
- **AND** the aggregate phase has completed before the finalize phase starts

#### Scenario: finalize is the last phase

- **WHEN** the finalize phase completes
- **THEN** no further ingestion, aggregation, or writing occurs for that run

### Requirement: Phase 1 prepares the run and the backend

The prepare phase SHALL create the run's data directories and the run's `building/` snapshot location — into which every snapshot database (the code graph and the knowledge store) is written — exactly once per run. No ingester SHALL create or reset the run's directories; in the ingest phase each ingester opens its own writer handle against the prepared `building/` location.

The prepare phase SHALL preflight that the ingester CLIs the run requires are available, and SHALL fail the run in the prepare phase — before any store is written — when a required CLI is missing.

#### Scenario: the run's directories are created once, by prepare

- **WHEN** a run begins and several language ingesters follow
- **THEN** the prepare phase creates the run directories and the `building/` location
- **AND** no ingester creates or resets them

#### Scenario: a missing ingester CLI fails the run before any store is written

- **WHEN** the run requires an ingester CLI that is not available
- **THEN** the run fails in the prepare phase
- **AND** no store has been written

### Requirement: short_id is partitioned by language

`short_id` SHALL be partitioned by source language: the high bits SHALL
encode the `Language` discriminant and the low bits a per-language counter.
`Language` is a closed, compile-time-known enum, so the partition set is
fixed at build time.

Each ingester SHALL own exactly one language partition and SHALL intern into
its own `IdRegistry` within that partition. There SHALL be no run-global
`IdRegistry` and no cross-ingester shared mutable state. The invariant SHALL
be one ingester per language per partition.

#### Scenario: ingesters intern independently

- **WHEN** two ingesters for different languages run concurrently
- **THEN** each assigns `short_id`s only within its own language partition
- **AND** neither reads or writes the other's interning state

#### Scenario: short_id partitions do not collide

- **WHEN** the code graph is assembled from all ingesters
- **THEN** every `short_id` is unique across languages by construction of the partition

### Requirement: The aggregate phase computes the rolled-up graph

The aggregate phase SHALL read the code graph back and write the rolled-up
`aggregate_*` / `analysis_*` tables. It SHALL run after every ingester has
completed and before the finalize phase.

#### Scenario: aggregation runs between ingest and finalize

- **WHEN** the ingest phase has completed
- **THEN** the aggregate phase computes the aggregate graph and writes the `aggregate_*` / `analysis_*` tables
- **AND** it completes before the finalize phase begins

### Requirement: Finalize publishes both stores atomically

The finalize phase SHALL build the run's search indexes and publish the run's snapshot so that data becomes visible only at the publish point: the `building/` directory is moved to its published run location and the `live` pointer is repointed to it. Every database the run produced — the code graph and the knowledge store — is published by this one atomic swap.

A run that fails before the finalize phase SHALL leave the previously published data intact and visible.

#### Scenario: data becomes visible only at the publish point

- **WHEN** a run is mid-ingest, with records already appended to `building/`
- **THEN** readers continue to see the previously published snapshot
- **AND** the new data becomes visible only when the `live` pointer is repointed

#### Scenario: a failed run leaves the previous snapshot intact

- **WHEN** a run fails before the finalize phase
- **THEN** the previously published data remains intact and visible

### Requirement: Ingester completion is tracked by task result

The orchestrator SHALL determine, for each language ingester, whether it finished cleanly or was truncated. A clean finish SHALL be the ingester task completing successfully after appending its last batch; a truncated ingester SHALL be the task ending with an error or a panic. The orchestrator SHALL distinguish the two from the task result, without an in-band stream marker.

#### Scenario: clean finish is distinguished from a crash

- **WHEN** an ingester appends all its records and its task completes successfully
- **THEN** the orchestrator records that ingester as cleanly completed

#### Scenario: a crashed ingester is detected as truncated

- **WHEN** an ingester's task ends with an error or panic before completing
- **THEN** the orchestrator reports the run as truncated rather than cleanly completed

### Requirement: The orchestrator registers one driver per enabled language

The orchestrator SHALL register the Swift JSONL indexer driver when
`[language.swift]` is enabled in configuration, as a sibling producer alongside the
C#, TypeScript, Rust, and Python drivers. The Swift driver SHALL reuse the existing
`JsonlIndexer` contract; when the Swift sidecar binary is absent the run SHALL
report the driver as unavailable (as for a missing C#/TS sidecar) rather than
failing the whole index.

#### Scenario: Swift driver registered when enabled

- **WHEN** configuration sets `[language.swift] enabled = true`
- **THEN** `configure_runner` registers a Swift JSONL driver in the runner

#### Scenario: Swift disabled by default

- **WHEN** no `[language.swift]` block enables it
- **THEN** no Swift driver is registered and Swift files are not indexed

#### Scenario: missing sidecar degrades gracefully

- **WHEN** Swift is enabled but the `kenn-swift` binary is not found
- **THEN** the run reports the Swift driver unavailable and other languages still
  index

### Requirement: HTML ingest runs as a parallel producer gated for connective resolution

HTML ingest SHALL run as an additional parallel producer during the ingest phase
(alongside code, markdown, and stylesheet ingest). Its connective steps —
`<a href>`/fragment link resolution, `html_id`↔`css_id` correspondence, and
`class=`/`id=` usage attribution — SHALL run as a step gated on completion of
code ingest and the CSS class registry, mirroring how stylesheet usage
resolution is gated: the code file nodes and the class/id registries must exist
before HTML edges can resolve against them. The gated step SHALL run before
finalize/publish.

#### Scenario: document nodes are produced in the parallel phase

- **WHEN** the ingest phase runs
- **THEN** HTML document nodes are produced in parallel with code/CSS ingest

#### Scenario: class usage resolution waits for the registry

- **WHEN** HTML `class=` usage attribution runs
- **THEN** it runs only after code ingest and the CSS class registry are complete

#### Scenario: id correspondence waits for css ids

- **WHEN** `html_id`↔`css_id` correspondence is computed
- **THEN** it runs after the CSS id nodes have been produced

### Requirement: Markdown ingest runs as a parallel ingest unit

During the ingest phase, the orchestrator SHALL run markdown ingestion as an
additional unit concurrent with the per-language code ingest units, streaming
its records through the same bounded channel to the DB writer.

#### Scenario: Markdown and code ingest concurrently

- **WHEN** a run includes both code and markdown roots
- **THEN** markdown ingestion proceeds concurrently with the code ingest units
  within the ingest phase

### Requirement: Markdown-to-code resolution is gated on code-ingest completion

The orchestrator SHALL run markdown-to-code link resolution as a step that
begins only after all code ingest units have completed and before
finalize/publish. Markdown-to-markdown resolution SHALL NOT be gated on this
barrier.

#### Scenario: Code links resolve after the barrier

- **WHEN** code ingest units are still running
- **THEN** markdown-to-code edges are not yet resolved
- **AND** once all code ingest units complete, the resolution step runs before
  the snapshot is published

#### Scenario: A run with no code still publishes markdown

- **WHEN** a run indexes only markdown roots (no code units)
- **THEN** the markdown graph is resolved and published without waiting on a
  code barrier

### Requirement: Stylesheet ingest runs as a parallel ingest unit

During the ingest phase, the orchestrator SHALL run stylesheet ingestion as an
additional unit concurrent with the per-language code ingest units, streaming its
records through the same bounded channel to the DB writer.

#### Scenario: Stylesheet and code ingest concurrently

- **WHEN** a run includes both code and stylesheet roots
- **THEN** stylesheet ingestion proceeds concurrently with the code ingest units
  within the ingest phase

### Requirement: CSS-internal and class-usage resolution have distinct gates

The two post-producer resolution steps have different dependencies and SHALL be
gated independently:

- **CSS-internal resolution** (`@use`/`@import`/`@forward` → `imports`;
  `@extend`/`composes` → `extends_rule`) connects stylesheet nodes only, so it
  SHALL be gated **only on the stylesheet producer** completing — it MAY run
  concurrently with code ingest, NOT behind the code barrier.
- **Class-usage mining** (`uses_css_class`) attaches a code node as the source
  endpoint, so it SHALL be gated on **all code ingest units** completing (the
  existing post-code barrier), in addition to the stylesheet producer.

Stylesheet parsing and the class registry SHALL NOT be gated on either barrier —
they are the producer. Both resolution steps run before finalize/publish.

#### Scenario: CSS-internal resolves without waiting for code

- **WHEN** the stylesheet producer has finished but code ingest is still running
- **THEN** CSS-internal (`imports`/`extends_rule`) edges MAY already resolve
- **AND** `uses_css_class` edges are not yet emitted

#### Scenario: Usage edges resolve after the code barrier

- **WHEN** code ingest units are still running
- **THEN** `uses_css_class` edges are not yet emitted
- **AND** once all code ingest units complete, the usage step runs before publish

#### Scenario: A run with no code still publishes stylesheets

- **WHEN** a run includes stylesheet roots but no code
- **THEN** the stylesheet corpus (nodes + CSS-internal edges) is published
- **AND** the usage step resolves against an empty code graph without failing

### Requirement: Producer registration is identical across all index entry paths

The set of producers enabled for an index run SHALL be derived from a **single
source of truth**, so that every enabled producer runs regardless of which entry
path (CLI `kenn index` or the workflow/MCP `index_workspace`) triggers the run.
Adding, removing, or configuring a producer SHALL take effect on all entry paths
from one edit; no entry path may register a different producer set than another
for the same config.

#### Scenario: an enabled producer runs on both entry paths

- **GIVEN** a config with `[language.markdown] enabled = true`
- **WHEN** an index run is triggered via the CLI **and** via the MCP/workflow path
- **THEN** both runs register the markdown producer and produce markdown nodes

#### Scenario: adding a producer cannot drift between paths

- **WHEN** a new producer is added to the index driver configuration
- **THEN** it is registered from the single shared configuration function used by
  every entry path, so no path silently omits it

### Requirement: Ingesters write records directly to per-language writers

In the ingest phase the orchestrator SHALL spawn one ingester per language. Each ingester SHALL parse its stream, intern into its own `short_id` partition, build records, and append them to the run's snapshot database through its own append surface. There SHALL be no single DB-writer thread and no ingester-to-writer record channel.

Concurrent ingesters MAY append to the same database. The writer handle SHALL be cheap to clone and SHALL serialize the appends its clones make, so a concurrent append is ordered rather than rejected: no ingester's writes are lost to a commit conflict, and none has to retry. There SHALL be one append surface per language ingester — not one per internal parser thread — so the concurrent-appender count equals the language count.

Each ingester SHALL accumulate records into batches before appending; memory in flight is therefore bounded by `(language count) × (batch size)`.

#### Scenario: concurrent ingesters produce the union of their records

- **WHEN** several language ingesters run concurrently and append directly
- **THEN** after all of them finish, the code graph contains the union of every ingester's records
- **AND** no ingester's writes are rejected or lost to a commit conflict

#### Scenario: in-flight memory is bounded by per-ingester batching

- **WHEN** ingesters produce records faster than they are appended
- **THEN** the records held in memory never exceed one batch per language ingester

