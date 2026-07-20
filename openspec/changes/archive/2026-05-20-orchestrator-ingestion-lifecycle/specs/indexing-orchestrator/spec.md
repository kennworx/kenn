## ADDED Requirements

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

The prepare phase SHALL create the run's data directories and construct the
storage backend — the redb code-graph database and the Lance temp build
store — exactly once per run. The backend SHALL be owned solely by the
DB-writer thread (see the bounded-channel requirement); no ingester SHALL
create, reset, or hold the backend.

The prepare phase SHALL preflight that the ingester CLIs the run requires are
available, and SHALL fail the run in the prepare phase — before any store is
written — when a required CLI is missing.

#### Scenario: backend created once, owned by the writer

- **WHEN** the prepare phase completes
- **THEN** the redb database and the Lance temp store exist
- **AND** only the DB-writer thread holds them; no ingester re-creates or resets them

#### Scenario: missing ingester CLI fails in prepare

- **WHEN** a required ingester CLI is not available on the system
- **THEN** the run fails during the prepare phase
- **AND** no code-graph or knowledge-store data has been written

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

### Requirement: Ingesters stream records to a single DB-writer over a bounded channel

In the ingest phase the orchestrator SHALL spawn one DB-writer thread and one
ingester per language. Each ingester SHALL parse its stream, intern into its
own partition, build records, and send them to the DB-writer over a bounded
channel that carries built records plus `Begin` / `End` markers.

The channel SHALL be bounded by **record count** — its capacity is a number
of records, so memory in flight is `capacity × record_size`. A full channel
SHALL apply backpressure by blocking the sending ingester.

The DB-writer thread SHALL be the sole owner of the redb database and the
Lance store. It SHALL accumulate channel records into batches and write them
to redb and to the Lance store; because there is exactly one writer, neither
store SHALL require a write lock against other ingesters.

#### Scenario: bounded channel caps in-flight memory

- **WHEN** ingesters produce records faster than the DB-writer consumes them
- **THEN** the number of records held in the channel never exceeds its configured capacity
- **AND** a producing ingester blocks on send until the DB-writer drains capacity

#### Scenario: concurrent ingesters produce the union of their records

- **WHEN** several language ingesters run concurrently
- **THEN** after all of them finish, the code graph contains the union of every ingester's records
- **AND** no ingester's writes are rejected or lost to lock contention

### Requirement: Begin and End markers track clean completion

Each ingester SHALL send a `Begin` marker before its records and an `End`
marker after its last record on a clean finish. The DB-writer SHALL treat a
channel that closed without a matching `End` for a started stream as a
**truncated** (crashed) ingester, distinct from a clean finish.

#### Scenario: clean finish is distinguished from a crash

- **WHEN** an ingester sends `Begin`, its records, then `End`
- **THEN** the DB-writer records that stream as cleanly completed

#### Scenario: a dropped ingester is detected as truncated

- **WHEN** an ingester's channel sender drops after `Begin` but with no `End`
- **THEN** the DB-writer reports the run as truncated rather than cleanly completed

### Requirement: The aggregate phase computes the rolled-up graph

The aggregate phase SHALL read the code graph back and write the rolled-up
`aggregate_*` / `analysis_*` tables. It SHALL run after every ingester has
completed and before the finalize phase.

#### Scenario: aggregation runs between ingest and finalize

- **WHEN** the ingest phase has completed
- **THEN** the aggregate phase computes the aggregate graph and writes the `aggregate_*` / `analysis_*` tables
- **AND** it completes before the finalize phase begins

### Requirement: Finalize publishes both stores atomically

The finalize phase SHALL compact the Lance knowledge store, build its
indexes, and publish both stores so that data becomes visible only at the
publish point: the Lance store via its directory swap into `.kenn/knowledge/`,
and the redb snapshot via the `building/ → snapshots/` move and `live`
symlink flip.

A run that fails before the finalize phase SHALL leave the previously
published data intact and visible.

#### Scenario: data is visible only after finalize

- **WHEN** the ingest and aggregate phases have completed but finalize has not
- **THEN** a reader still observes the previously published snapshot, not the in-flight run

#### Scenario: a run that fails before finalize leaves prior data intact

- **WHEN** a run fails during the prepare, ingest, or aggregate phase
- **THEN** the previously published code graph and knowledge store remain intact and queryable
