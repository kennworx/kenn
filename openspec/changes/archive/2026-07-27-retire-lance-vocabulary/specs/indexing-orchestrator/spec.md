## RENAMED Requirements

- FROM: `### Requirement: Ingesters write records directly to per-language Lance writers`
- TO: `### Requirement: Ingesters write records directly to per-language writers`

## MODIFIED Requirements

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
