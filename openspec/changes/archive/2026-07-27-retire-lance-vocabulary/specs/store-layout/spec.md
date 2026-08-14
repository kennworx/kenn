## MODIFIED Requirements

### Requirement: Derived state is runs-centric; no separate snapshots directory

Every index pass SHALL write its output into a single directory
`<derived_root>/runs/{id}/` containing the raw inputs, all snapshot
databases, and per-run metadata. On successful completion, the
`<derived_root>/live` pointer is repointed to that run.

A `runs/{id}/` directory SHALL contain:

- `tmp/` — scratch for atomic-rename sidecar writes (see
  "Sidecar writes are atomic and lock-free" requirement). Empty
  in steady state.
- `*.scip` — one file per language with raw SCIP indexes
- `*.jsonl` — one file per language with JSONL frame inputs
- `code.db` — the code-graph snapshot database: `symbols`, `defs`,
  `files`, the per-kind edge data, and the `aggregate_*` /
  `analysis_*` data.
- `vector.db` — the search database: the row text, its FTS5 indexes,
  and the `vec0` vector table. Its `embedding` data is populated at
  index time from the committed sidecar at `<vectors_root>/code/`.
  There is no separate vectors dataset.
- `report.json` — indexer outcome metadata
- `meta.json` — snapshot stamp (timestamp, fingerprints, etc.)

The findings store SHALL NOT be per-run: it lives at the derived root
as `findings.db`, with the committed per-finding records under
`<committed_root>/findings/`. A run directory SHALL NOT contain a
findings database.

Run ids SHALL be ISO-8601 UTC timestamps with second precision and
colons replaced by dashes (`YYYY-MM-DDTHH-MM-SSZ`) so they sort
lexically and remain filesystem-safe across platforms.

The store SHALL NOT maintain a separate `snapshots/` directory.
Rollback re-points the `live` pointer to a prior `runs/{id}/`.

`<derived_root>/live` SHALL be a regular UTF-8 text file whose sole
contents are the target's path — NOT a symlink, junction, or any
other filesystem link type. A link cannot be created unprivileged on
every supported platform (Windows `symlink_dir` requires
Administrator or Developer Mode), and the alternative that can
(a directory junction) has no atomic replace, which readers depend
on.

The `live` target SHALL be a relative path (`runs/{id}`, not an
absolute path) so that it remains valid when `derived_root` is moved
on disk.

Replacement SHALL be atomic: the file is written to a temporary name
in the same directory and renamed over `live`. A concurrent reader
SHALL observe either the previous target or the new one, and SHALL
NEVER observe a missing, empty, or partially written pointer.

#### Scenario: a successful index pass creates one runs directory

- **WHEN** `kenn index` runs against a workspace
- **THEN** a directory `<derived_root>/runs/{new_id}/` is created
- **AND** the run's `code.db` and `vector.db` live directly under
  `<derived_root>/runs/{new_id}/`
- **AND** per-language `.scip` and `.jsonl` files live at
  `<derived_root>/runs/{new_id}/`
- **AND** `<derived_root>/live` is a regular file whose contents are
  `runs/{new_id}` (relative path, no trailing slash)

#### Scenario: the findings store is not part of a run

- **WHEN** an index pass completes
- **THEN** the run directory contains no findings database
- **AND** `findings.db` remains at the derived root, unaffected by
  the `live` repoint

### Requirement: Deferred runs-centric placements have direct test coverage

The deferred runs-centric layout claims SHALL hold true in code and SHALL be backed by direct tests: the indexer SHALL be exercised against per-language JSONL files written at `<derived_root>/runs/{id}/{lang}.jsonl` (not at multi-file driver-specific names); the findings store SHALL round-trip writes and reads against `findings.db` at the derived root across an indexer pass; the deprecated path accessors `findings_local_dir()` and `embed_lock_path()` SHALL be removed from `Layout`; the `embed-locks/` directory SHALL no longer be created by any code path. If a workspace's `<derived_root>` already contains a stale `embed-locks/` directory from a prior layout, the indexer MAY sweep it at startup; this MUST NOT block normal operation.

#### Scenario: indexer reads JSONL from the runs-centric path

- **WHEN** an indexer pass runs against a workspace with a
  language driver that emits JSONL frames
- **THEN** the driver writes its frames to
  `<derived_root>/runs/{id}/{lang}.jsonl` (one file per language)
- **AND** the indexer ingests those frames from that path
- **AND** no `kenn-dotnet-stream-*.jsonl` or other multi-file
  driver-specific name is produced

#### Scenario: findings round-trip across an indexer pass

- **WHEN** a finding is written, an indexer pass runs, and the finding
  is read back
- **THEN** the read returns the written record
- **AND** the store it round-tripped through is `findings.db` at the
  derived root, which no `live` repoint moved or replaced

#### Scenario: the deprecated accessors and embed-locks are gone

- **WHEN** the `Layout` API is inspected
- **THEN** it exposes no `findings_local_dir()` and no `embed_lock_path()`
- **AND** no code path creates an `embed-locks/` directory
