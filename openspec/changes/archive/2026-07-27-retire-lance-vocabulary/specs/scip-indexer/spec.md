## MODIFIED Requirements

### Requirement: Run report

The system MUST persist a per-run report containing counts of
documents, symbols, occurrences, and relationships processed, a
list of failed projects with diagnostics, and a status field of
`success | partial | failed`. The report MUST be persisted
alongside the produced data-model records.

The `documents` count SHALL equal the number of distinct source
file paths the indexer visited in this run — not zero, not the
number of `FileRecord` rows emitted to the `files` table.
A path that appears in multiple SCIP `Document` messages (e.g. one
per project in a multi-csproj solution) MUST be counted exactly
once.

#### Scenario: Querying the latest run for a unit

- **WHEN** a consumer asks for the latest run report for `App.sln`
- **THEN** the system MUST return the most recent report including its status, counts, and any failures

#### Scenario: documents count is non-zero on a non-empty workspace

- **WHEN** `kenn index` runs against a workspace containing N
  source files of an enabled language and the run completes
  successfully
- **THEN** `meta.json["documents"]` SHALL be ≥ 1
- **AND** `meta.json["documents"]` SHALL equal the number of
  distinct source file paths the indexer visited (deduplicated
  across SCIP `Document` messages that repeat a path)

#### Scenario: documents count survives intern dedup

- **WHEN** the SCIP stream contains two `Document` messages with
  the same `relative_path` (e.g. the same file emitted from two
  csproj projects)
- **THEN** the `files` table MUST contain exactly one
  `FileRecord` for that path (existing intern-dedup behaviour
  preserved)
- **AND** `meta.json["documents"]` MUST count that path once,
  regardless of whether the second `Document` produced a
  `FileRecord` (i.e., the dedup gate on `FileRecord` emission
  MUST NOT silently suppress the file-count increment)
