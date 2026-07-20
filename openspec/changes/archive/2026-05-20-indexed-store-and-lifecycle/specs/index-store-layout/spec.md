## ADDED Requirements

### Requirement: Per-location store directory

Each indexable location (a repo or a worktree) SHALL maintain its index store under a `.kenn/` directory at its root. The directory MUST contain the following entries when populated:

- `live` — a symbolic link pointing into `snapshots/<timestamp>/`
- `snapshots/` — directory holding zero or more immutable snapshot directories named by ISO-8601 UTC timestamp (e.g., `2026-05-01T15-30-00Z`)
- `building/` — directory used exclusively by an in-progress indexing run; absent during steady state
- `runs/<run-id>/` — directory per indexer run holding a `report.json` plus any per-run diagnostics; retained even after a snapshot is GC'd, subject to a separate retention policy

#### Scenario: Steady-state layout

- **WHEN** the workspace is in steady state with one successful indexer run completed
- **THEN** `.kenn/` MUST contain `live`, `snapshots/T0/`, `runs/<run-id>/`
- **AND** `.kenn/` MUST NOT contain `building/`

#### Scenario: First-time init has no snapshot

- **WHEN** `kenn init` has been run but `kenn index` has not
- **THEN** `.kenn/` MUST exist
- **AND** `live` MUST NOT exist
- **AND** `snapshots/` MAY exist but be empty

### Requirement: Snapshot directories are immutable

Once a snapshot directory under `snapshots/` exists with `live` pointing at it (or with `live` having pointed at it), the contents of that directory SHALL NOT be modified by any subsequent operation. The only legal operation on a snapshot directory after creation is recursive deletion during GC.

#### Scenario: Indexer never writes into a published snapshot

- **WHEN** an indexer run begins while `live → snapshots/T0`
- **THEN** the run MUST write only into `building/`, never into `snapshots/T0/`

#### Scenario: A query reading a snapshot during GC of a different snapshot

- **WHEN** a reader opens files in `snapshots/T1` (the current `live`)
- **AND** GC is concurrently deleting `snapshots/T_old`
- **THEN** the reader MUST be unaffected

### Requirement: `live` symlink targets a snapshot

The `live` entry SHALL be a symbolic link (POSIX symlink; on platforms where symlinks are unavailable, see *Atomic flip portability* in `index-lifecycle`). Its target SHALL be a relative path of the form `snapshots/<timestamp>` resolving inside the same `.kenn/` directory. Readers MUST resolve queries against the directory `live` points to at the moment they open the DB; in-flight reads MUST NOT be affected by a subsequent flip (see lifecycle invariants).

#### Scenario: Reader opens DB through the symlink

- **WHEN** a reader resolves the path `.kenn/live` and opens the DB
- **THEN** the reader MUST observe a consistent view of the snapshot live points to at open time

### Requirement: Run reports persist independently of snapshots

A `runs/<run-id>/report.json` MUST be written before the corresponding snapshot is GC'd, and MUST remain readable for diagnostic purposes for a configurable retention window (default: 30 days), independent of whether the associated snapshot still exists.

#### Scenario: Snapshot GC'd, report retained

- **WHEN** `snapshots/T_old` is GC'd
- **THEN** `runs/<run-id>/report.json` for that run MUST still exist and be readable until the report retention window elapses
