## ADDED Requirements

### Requirement: Ingesters write records directly to per-language Lance writers

In the ingest phase the orchestrator SHALL spawn one ingester per language. Each ingester SHALL parse its stream, intern into its own `short_id` partition, build records, and append them to the run's Lance datasets through its own writer. There SHALL be no single DB-writer thread and no ingester-to-writer record channel.

Concurrent ingesters MAY append to the same Lance dataset. Lance's default optimistic-concurrency commit guard SHALL resolve concurrent appends: an `Append` is conflict-free with another `Append`, so a writer that loses a manifest race SHALL rebase and retry rather than fail. There SHALL be one writer per language ingester — not one per internal parser thread — so the concurrent-committer count equals the language count.

Each ingester SHALL accumulate records into batches before appending; memory in flight is therefore bounded by `(language count) × (batch size)`.

#### Scenario: concurrent ingesters produce the union of their records

- **WHEN** several language ingesters run concurrently and append directly
- **THEN** after all of them finish, the code graph contains the union of every ingester's records
- **AND** no ingester's writes are rejected or lost to a commit conflict

#### Scenario: in-flight memory is bounded by per-ingester batching

- **WHEN** ingesters produce records faster than they are appended
- **THEN** the records held in memory never exceed one batch per language ingester

### Requirement: Ingester completion is tracked by task result

The orchestrator SHALL determine, for each language ingester, whether it finished cleanly or was truncated. A clean finish SHALL be the ingester task completing successfully after appending its last batch; a truncated ingester SHALL be the task ending with an error or a panic. The orchestrator SHALL distinguish the two from the task result, without an in-band stream marker.

#### Scenario: clean finish is distinguished from a crash

- **WHEN** an ingester appends all its records and its task completes successfully
- **THEN** the orchestrator records that ingester as cleanly completed

#### Scenario: a crashed ingester is detected as truncated

- **WHEN** an ingester's task ends with an error or panic before completing
- **THEN** the orchestrator reports the run as truncated rather than cleanly completed

## MODIFIED Requirements

### Requirement: Phase 1 prepares the run and the backend

The prepare phase SHALL create the run's data directories and the run's `building/` snapshot location — into which every Lance dataset (the code graph and the knowledge store) is written — exactly once per run. No ingester SHALL create or reset the run's directories; in the ingest phase each ingester opens its own writer against the prepared `building/` location.

The prepare phase SHALL preflight that the ingester CLIs the run requires are available, and SHALL fail the run in the prepare phase — before any store is written — when a required CLI is missing.

#### Scenario: backend created once in prepare

- **WHEN** the prepare phase completes
- **THEN** the run's `building/` snapshot location exists
- **AND** no ingester re-creates or resets it

#### Scenario: missing ingester CLI fails in prepare

- **WHEN** a required ingester CLI is not available on the system
- **THEN** the run fails during the prepare phase
- **AND** no code-graph or knowledge-store data has been written

### Requirement: Finalize publishes both stores atomically

The finalize phase SHALL compact the run's Lance datasets, build their indexes, and publish the run's snapshot so that data becomes visible only at the publish point: the `building/` directory is moved to `snapshots/<timestamp>/` and the `live` symlink is flipped. Every Lance dataset the run produced — the code graph and the knowledge store — is published by this one atomic swap.

A run that fails before the finalize phase SHALL leave the previously published data intact and visible.

#### Scenario: data is visible only after finalize

- **WHEN** the ingest and aggregate phases have completed but finalize has not
- **THEN** a reader still observes the previously published snapshot, not the in-flight run

#### Scenario: a run that fails before finalize leaves prior data intact

- **WHEN** a run fails during the ingest or aggregate phase
- **THEN** the previously published snapshot remains live and readable

## REMOVED Requirements

### Requirement: Ingesters stream records to a single DB-writer over a bounded channel

**Reason**: redb required a single synchronous writer owned by one thread, so the orchestrator funnelled every ingester's records through a bounded channel into that one DB-writer. With redb retired and Lance's async, optimistic-concurrency writer in its place, ingesters append directly and concurrently — there is no single DB-writer thread and no channel.

**Migration**: Replaced by the added requirement "Ingesters write records directly to per-language Lance writers". Backpressure, previously the channel's record-count bound, is now the per-ingester batch bound.

### Requirement: Begin and End markers track clean completion

**Reason**: `Begin` / `End` markers were records on the ingester-to-DB-writer channel. That channel is removed with redb.

**Migration**: Replaced by the added requirement "Ingester completion is tracked by task result" — the orchestrator reads each ingester task's success or failure instead of matching in-band stream markers.
