## ADDED Requirements

### Requirement: Snapshot-and-swap lifecycle

The store SHALL transition through a fixed state machine: `Steady(T_n) → Indexing(T_n, building) → Steady(T_{n+1})`. While in `Indexing`, readers continue to query against `Steady(T_n)`'s snapshot. The transition to `Steady(T_{n+1})` SHALL be atomic from the reader's perspective.

#### Scenario: Reader during indexing observes only the previous snapshot

- **WHEN** an indexer run is mid-execution writing into `building/`
- **AND** a reader executes a query at any point during the run
- **THEN** the query MUST be served from the previous `live` snapshot
- **AND** the query MUST NOT observe partial data from `building/`

### Requirement: Atomic publish

On successful completion of an indexer run, the store SHALL publish the new snapshot via these steps in order: (1) `fsync` the contents of `building/`, (2) rename `building/` → `snapshots/<new-timestamp>/`, (3) atomically replace the `live` symlink to point at the new snapshot. After step 3, in-flight readers continue their work against their previously opened snapshot; new readers see the new snapshot.

#### Scenario: Successful flip

- **WHEN** the indexer run finishes successfully and the store publishes
- **THEN** `live` MUST point at `snapshots/<new-timestamp>/`
- **AND** `building/` MUST NOT exist
- **AND** the previous snapshot directory MUST still exist (subject to GC policy)

#### Scenario: Crash during publish

- **WHEN** the process crashes between step (2) and step (3)
- **THEN** on next startup the store MUST detect the orphan snapshot directory (no `live` pointing at it and not in the GC retention) and either complete the flip (if the run report is `Success`) or quarantine the orphan with a structured warning

### Requirement: Atomic flip portability

The atomic-flip step SHALL use a filesystem primitive that is atomic on the target platform: on POSIX systems, a `rename(2)` of a temporary symlink onto `live`. The store MUST document the supported platforms; non-POSIX platforms (Windows) are out of scope for v1 unless a documented atomic equivalent is implemented.

#### Scenario: POSIX atomic flip

- **WHEN** the store flips `live` on Linux or macOS
- **THEN** the implementation MUST use `rename(tmplink, live)` where `tmplink` is a fresh symlink in the same directory

### Requirement: Failed-run isolation

If an indexer run reports `Failed` (its produced run report) or the run process crashes/aborts before producing a complete report, the store SHALL NOT publish a new snapshot. The `building/` directory SHALL be deleted; `live` SHALL remain unchanged.

#### Scenario: Indexer process crashes mid-run

- **WHEN** the indexer process is killed during a run
- **THEN** on next startup the store MUST observe `building/` exists with no completed report
- **AND** the store MUST delete `building/`
- **AND** `live` MUST be unchanged from before the run started

#### Scenario: Run completes with status Failed

- **WHEN** the indexer reports `Failed` (e.g., scip-dotnet itself crashed)
- **THEN** the store MUST NOT flip
- **AND** `building/` MUST be deleted
- **AND** the run report MUST be persisted under `runs/<run-id>/`

### Requirement: Partial-run policy

If an indexer run reports `Partial` (some projects failed but data was produced for others), the store SHALL publish the snapshot. A `Partial` flip SHALL emit a warning in the post-flip metric report identifying the failed projects.

#### Scenario: Partial run flips with warning

- **WHEN** an indexer run reports `Partial` with 3 of 100 projects failed
- **THEN** the store MUST flip to the new snapshot
- **AND** the post-flip metric report MUST list the 3 failed projects

### Requirement: GC policy — keep current and previous

The store SHALL retain at most two snapshots: the one currently pointed at by `live` and the immediately previous one (the rollback target). After a successful flip, snapshots older than the new previous one SHALL be scheduled for deletion. Deletion SHALL be performed on a background task and MUST NOT interfere with readers of any retained snapshot.

#### Scenario: Three flips in sequence

- **WHEN** snapshots are created in order T0, T1, T2 with successful flips
- **THEN** after the flip to T2, only T1 and T2 MUST remain on disk
- **AND** T0 MUST be deleted (eventually; deletion is asynchronous)

#### Scenario: GC interrupted by crash

- **WHEN** the process crashes during deletion of an old snapshot
- **THEN** on next startup the store MUST resume deletion of any non-retained snapshot directories

### Requirement: Quality-metric report on flip

On every successful flip, the store SHALL produce a metric comparison between the new and previous snapshots covering at minimum: document count, symbol count, definition count, edge count, failed-project count, per-project document counts. The report SHALL identify regressions exceeding configurable thresholds (default: 10 % decrease in any per-snapshot count) as warnings. Warnings SHALL NOT block the flip; they SHALL be persisted in the new run's `report.json` and surfaced via `kenn status`.

#### Scenario: New build has 30 % fewer documents

- **WHEN** a new snapshot has 70 documents and the previous had 100
- **THEN** the post-flip metric report MUST contain a `regression` warning identifying the document-count drop
- **AND** the flip MUST NOT be blocked

#### Scenario: New build is comparable to previous

- **WHEN** all metric deltas are within ±10 %
- **THEN** the post-flip metric report MUST contain no regression warnings

### Requirement: Manual rollback command

The store SHALL support a `kenn rollback` operation that atomically flips `live` to point at the previous snapshot (the GC retention target). After rollback, the previously-current snapshot becomes the new "previous" and remains retained. Rollback SHALL fail with a clear error if no previous snapshot is retained.

#### Scenario: Rollback after a bad build

- **WHEN** `live → snapshots/T_bad` and `snapshots/T_good` is retained as the previous
- **AND** the user runs `kenn rollback`
- **THEN** `live` MUST atomically flip to `snapshots/T_good`
- **AND** `snapshots/T_bad` MUST become the new "previous" (still retained until next flip)

#### Scenario: Rollback with no previous snapshot

- **WHEN** only one snapshot is retained
- **AND** the user runs `kenn rollback`
- **THEN** the command MUST exit with a non-zero status and an error message indicating no rollback target is available
- **AND** `live` MUST be unchanged

### Requirement: One-writer invariant

At most one indexer run SHALL be in progress per `.kenn/` directory at a time. The store SHALL enforce this with an exclusive file lock on a designated lock file (e.g., `.kenn/index.lock`); a second indexer invocation while another is running MUST exit with a clear error message indicating the lock is held.

#### Scenario: Concurrent index attempts

- **WHEN** an indexer run is active and a second `kenn index` is invoked
- **THEN** the second invocation MUST exit with a non-zero status and a message identifying the holding PID and start time
- **AND** the second invocation MUST NOT touch `building/` or `snapshots/`
