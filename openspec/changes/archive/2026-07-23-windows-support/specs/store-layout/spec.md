## MODIFIED Requirements

### Requirement: Derived state is runs-centric; no separate snapshots directory

Every index pass SHALL write its output into a single directory
`<derived_root>/runs/{id}/` containing the raw inputs, all Lance
datasets, and per-run metadata. On successful completion, the
`<derived_root>/live` pointer is repointed to that run.

A `runs/{id}/` directory SHALL contain:

- `tmp/` — scratch for atomic-rename sidecar writes (see
  "Sidecar writes are atomic and lock-free" requirement). Empty
  in steady state.
- `*.scip` — one file per language with raw SCIP indexes
- `*.jsonl` — one file per language with JSONL frame inputs
- `lance/` — every Lance dataset for this run: the code graph
  (`knowledge`, `aggregate_*`, `analysis_*`, `files`, `defs`,
  `edges`, etc.) and the findings local mirror (`lance/findings/`).
  `knowledge.lance` retains its `embedding` column, populated at
  index time from the committed sidecar at `<vectors_root>/code/`;
  the ANN index is built on that column. No separate vectors-Lance
  dataset.
- `report.json` — indexer outcome metadata
- `meta.json` — snapshot stamp (timestamp, fingerprints, etc.)

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
- **AND** every Lance dataset for the run lives under
  `<derived_root>/runs/{new_id}/lance/`
- **AND** per-language `.scip` and `.jsonl` files live at
  `<derived_root>/runs/{new_id}/`
- **AND** `<derived_root>/live` is a regular file whose contents are
  `runs/{new_id}` (relative path, no trailing slash)
- **AND** no `<derived_root>/snapshots/` directory exists

#### Scenario: rollback repoints live to a prior run

- **WHEN** the workspace has runs A (older) and B (active),
  with `live` containing `runs/B`
- **AND** the user invokes `kenn rollback`
- **THEN** `live` is atomically repointed to `runs/A`
- **AND** `runs/B/` remains on disk until the retention sweep
  removes it

#### Scenario: a concurrent reader never observes a partial pointer

- **WHEN** a reader resolves `live` repeatedly while an index pass
  publishes a new run
- **THEN** every read resolves to a run directory that exists
- **AND** no read observes an absent, empty, or truncated `live`

#### Scenario: a store written before the pointer file is not served

- **WHEN** `live` is a symlink left by an older kenn version
- **THEN** resolution fails rather than following it
- **AND** the caller treats the workspace as having no live run and
  reindexes, per the documented no-migration policy

