## ADDED Requirements

### Requirement: Explicit-invocation staleness signal

The store SHALL accept an unconditional reindex request via `kenn index`. When invoked explicitly, the store MUST start a new indexer run regardless of any other staleness signal, subject only to the one-writer invariant.

#### Scenario: Explicit index with no detected changes

- **WHEN** the user runs `kenn index`
- **AND** the git-aware skip would otherwise skip the run
- **THEN** the indexer MUST run anyway

### Requirement: Git-aware reindex skip

When `kenn index` is invoked without an unconditional flag (e.g., `--force`), and the workspace is inside a git repository, the store SHALL compute a fast staleness key and skip the run if it matches the key recorded in the current `live` snapshot's run report. The staleness key SHALL be the tuple `(git_head_commit, dirty_file_summary)` where `dirty_file_summary` is a sorted list of `(path, content_xxhash)` over files reported by `git status --porcelain` that match indexable extensions configured for the workspace.

#### Scenario: HEAD and dirty files unchanged since last run

- **WHEN** `kenn index` runs without `--force`
- **AND** `git rev-parse HEAD` matches the previous snapshot's recorded HEAD
- **AND** the dirty-file summary matches
- **THEN** the indexer MUST NOT run
- **AND** the command MUST exit successfully with a message identifying the skip reason

#### Scenario: HEAD changed (branch switch with no edits)

- **WHEN** `kenn index` runs after `git checkout` of a different branch
- **AND** the dirty file set is empty on both sides
- **THEN** the staleness key MUST mismatch
- **AND** the indexer MUST run

#### Scenario: Dirty file edited

- **WHEN** the user has edited one tracked file and runs `kenn index`
- **THEN** the staleness key MUST mismatch (new file content hash)
- **AND** the indexer MUST run

#### Scenario: Workspace not a git repository

- **WHEN** the workspace root is not in a git repository
- **THEN** the staleness key MUST be considered always-mismatching (no skip)
- **AND** every `kenn index` invocation MUST run the indexer

### Requirement: Staleness key persistence

Each successful run's report SHALL include the staleness key that was current at the start of the run. The store SHALL read the current `live` snapshot's report to retrieve the key for comparison on subsequent invocations.

#### Scenario: Snapshot report includes staleness key

- **WHEN** an indexer run completes successfully
- **THEN** `runs/<run-id>/report.json` MUST contain `staleness_key: { git_head, dirty_files: [...] }`

### Requirement: Force-reindex flag

The CLI SHALL expose a `--force` flag to `kenn index` that bypasses the git-aware skip even when the staleness key matches.

#### Scenario: User suspects a previous build was bad

- **WHEN** the user runs `kenn index --force`
- **AND** the staleness key matches
- **THEN** the indexer MUST still run
