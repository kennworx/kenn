# store-layout Specification

## Purpose
TBD - created by archiving change config-driven-store-layout. Update Purpose after archive.
## Requirements
### Requirement: A single Layout resolves every store path

A single `Layout` value, resolved once from configuration, SHALL be the sole
source of every store path. No component SHALL join store path segments —
`.kenn`, `local/`, `findings/`, `vectors/`, `snapshots/`, `scip-*.scip`, and the
rest — on its own; every path SHALL come from a `Layout` accessor.

#### Scenario: paths come only from Layout

- **WHEN** any component — indexer, store, findings store, or MCP server —
  needs a store path
- **THEN** it obtains that path from a `Layout` accessor
- **AND** no store path segment is hardcoded outside the layout module

### Requirement: The store separates committed and derived data into named roots

The store SHALL split its on-disk state into three roots, each
resolved once at `Layout::resolve()`:

- `source_root` — the workspace root (where the source code lives).
- `committed_root` — always `<source_root>/.kenn`, git-tracked,
  not relocatable.
- `derived_root` — gitignored, throwaway, rebuilt by `kenn index`,
  relocatable via `[layout] derived_root` (relative path, absolute
  path, or the keyword `"global"`).

The committed root holds:

- `findings/{id}.md` — finding records (source of truth), markdown with
  immutable YAML frontmatter (`id`, `tags`, `parent_ids`, `created_at`) and a
  prose body.
- `findings/{id}.anchor.jsonl` — the per-finding append-only anchor + liveness
  event log (mutable, mergeable).
- `vectors/code/` — committed code embedding sidecar.
- `vectors/findings/` — committed findings embedding sidecar.
- `.gitignore` — excludes `local/`.

The derived root holds:

- `runs/{id}/` — one directory per index pass (see "runs-centric
  derived state" requirement), including the snapshot-local `overview.md`
  orientation file written by `kenn index`.
- `live` — symlink to the active run.
- `index.lock`, `findings.lock`, `readers/` — store-wide
  bookkeeping. The `embed-locks/` directory is no longer
  required (content-addressed naming + per-writer unique tmp
  filenames replace the per-sidecar advisory lock).

#### Scenario: default layout for a fresh workspace

- **WHEN** `Layout::default_for(<source>)` is called on a workspace
  with no `kenn.toml`
- **THEN** `committed_root` resolves to `<source>/.kenn`
- **AND** `derived_root` resolves to `<source>/.kenn/local`
- **AND** `vectors_root` resolves to `<source>/.kenn/vectors`
- **AND** `code_vectors_dir()` resolves to `<source>/.kenn/vectors/code`
- **AND** `findings_vectors_dir()` resolves to
  `<source>/.kenn/vectors/findings`

#### Scenario: derived_root override relocates only derived state

- **WHEN** `kenn.toml` sets `[layout] derived_root = "global"`
- **THEN** `derived_root` resolves to an XDG cache path keyed by
  the repo id
- **AND** `committed_root` still resolves to `<source>/.kenn`
- **AND** `vectors_root` still resolves to `<source>/.kenn/vectors`
  (the vectors location is independent of `derived_root`)

#### Scenario: a finding's record and anchor log are committed, the snapshot is derived

- **WHEN** a finding is flushed and `kenn index` runs
- **THEN** `findings/{id}.md` and `findings/{id}.anchor.jsonl` are under the
  committed root and git-tracked
- **AND** the run's `overview.md` is under the derived root and gitignored

#### Scenario: paths come only from Layout

- **WHEN** any component needs the path of a finding record, its anchor log, or
  the snapshot overview
- **THEN** it obtains that path from a `Layout` accessor
- **AND** no such path segment is hardcoded outside the layout module

### Requirement: Vectors location is independently configurable

The committed vectors root SHALL be relocatable via the `[vectors] location`
config setting, which accepts: a **relative path**, an **absolute path**, or the
keyword `"global"` (an XDG cache path keyed by the repo id).

A relative path SHALL resolve against the **main worktree** (git root), not the
per-worktree `source_root`, so that every worktree of one repository resolves a
relative location to the same directory. The main worktree is discovered via the
in-process git backend (`git::main_worktree`); outside a git working tree the
relative path SHALL resolve against `source_root` (prior behavior). When
`source_root` *is* the main worktree, its own path spelling is preserved so
resolved layouts stay byte-identical to the pre-change default.

When set, every sidecar directory — the per-generation namespaces and the
legacy flat `code/`/`findings/` dirs — SHALL resolve under the new location.

The default — when `[vectors] location` is unset — SHALL be the git-root-relative
shared vectors subdir, so linked worktrees share a vector cache out of the box;
outside a git tree it SHALL remain `<committed_root>/vectors` (prior behavior).

#### Scenario: a relative location is shared across worktrees

- **GIVEN** two worktrees of one repository, both with `[vectors] location = "vectors"`
- **WHEN** each resolves its layout
- **THEN** both resolve `vectors_root` to the same `<main-worktree>/vectors`

#### Scenario: relative location outside a git tree keeps prior behavior

- **WHEN** a relative `[vectors] location` is set in a non-git directory
- **THEN** it resolves against `source_root`, unchanged

#### Scenario: default location is shared across worktrees

- **GIVEN** a linked worktree with no `[vectors] location` set
- **WHEN** it resolves its layout
- **THEN** `vectors_root` resolves under the main worktree's shared vectors subdir
- **AND** a second worktree resolves to the same directory

#### Scenario: absolute override still moves both sidecars verbatim

- **WHEN** `[vectors] location = "/mnt/shared/kenn-vectors"`
- **THEN** `vectors_root` resolves to `/mnt/shared/kenn-vectors`
- **AND** `code_vectors_dir()` / `findings_vectors_dir()` resolve under it

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

### Requirement: Vector sidecar files are content-addressed and append-only

The sidecar SHALL store cached vectors as content-addressed
files that are never rewritten. Each sidecar directory
(`<vectors_root>/code/` and `<vectors_root>/findings/`) SHALL
contain only the following entries:

- `manifest.toml` — format and model stamp for that sidecar
  (recipe tag differs: `sig-lf-doc/v1` for code, `finding-text/v1`
  for findings).
- `pack-{content_hash}.bin` — CI-produced, committed via git,
  the canonical record of cached vectors.
- `seg-{content_hash}.bin` — workspace-local, gitignored,
  produced by dev incremental embeds since the last `git pull`.
- `.tmp/` — present only when `Layout::writer_tmp_dir()` falls
  back to a vectors-co-located scratch dir because
  `<vectors_root>` is on a different filesystem from
  `<derived_root>` (see "Sidecar writes are atomic and
  lock-free"). Holds short-lived `*.tmp` files only.

Both prefixes SHALL share the same on-disk byte layout (the
prefix distinguishes only producer role, not file format):

- 16-byte fixed header: magic (`b"KVS2"`, 4 bytes), packed
  `ver_quant` u32 (low byte = quant code, upper 3 bytes = format
  version), `dim` u32, `count` u32. All little-endian.
- `count × 8` bytes: fingerprints, sorted ascending u64
  little-endian.
- Payload: `count` entries, each consisting of an f32 scale (LE)
  followed by `dim` int8 codes.

`count` SHALL be ≤ `MAX_ENTRIES = (4096 − 16) / 8 = 510`, so a
full file's header fits within one 4 KB OS page. Writers
SHALL reject encoding more than `MAX_ENTRIES` entries into a
single file.

The filename's `content_hash` SHALL be `xxh3_64(file_bytes)`
formatted as 16-character lowercase hexadecimal. Readers MAY
verify and reject files whose recomputed content hash does not
match their filename.

Writers SHALL sort entries ascending by fingerprint before
encoding. Two writers producing the same `(fingerprint, vector)`
set SHALL produce byte-identical files (and therefore identical
filenames).

Sidecar files SHALL NOT be rewritten in place. The set of files
in each sidecar directory grows by addition only. The only
deletion path is a future `kenn gc` maintenance command, out of
scope for this change.

#### Scenario: a fresh CI build produces one new pack file per commit

- **WHEN** CI runs `kenn index --repack` at a commit that adds
  N new embeddable symbols (N ≤ 510, the per-file cap)
- **THEN** the existing committed `pack-*.bin` files are
  unchanged
- **AND** exactly one new file `pack-{content_hash}.bin` is added
  to `<vectors_root>/code/`
- **AND** the new file contains the N newly-embedded fingerprint
  and vector entries, sorted ascending by fingerprint
- **AND** the content hash in the filename equals
  `xxh3_64(file_bytes)` in lowercase hex

#### Scenario: dev local index appends seg files

- **WHEN** a developer runs `kenn index` (no `--repack` flag)
  against a worktree with new symbols not present in any
  committed `pack-*.bin`
- **THEN** the newly-embedded vectors are written as
  `seg-{content_hash}.bin` files in `<vectors_root>/code/`
- **AND** no `pack-*.bin` file is modified or deleted
- **AND** the `seg-*.bin` files match the patterns in the
  workspace `.gitignore` (i.e., not staged for commit by default)

#### Scenario: --repack promotes existing seg files to packs

- **WHEN** `<vectors_root>/code/` contains `seg-X.bin` and
  `seg-Y.bin` (dev-local files from an earlier non-`--repack`
  run)
- **AND** the user runs `kenn index --repack`
- **THEN** after the run, `<vectors_root>/code/` contains
  `pack-X.bin` and `pack-Y.bin` with byte-identical content to
  the original seg files
- **AND** no `seg-*.bin` file remains in `<vectors_root>/code/`
- **AND** if a `pack-X.bin` already existed before the run
  (same content hash as the seg), the seg-X.bin is unlinked
  without overwriting the existing pack file

#### Scenario: reader applies pack-over-seg precedence on duplicate fp

- **WHEN** `<vectors_root>/code/` contains a `seg-{A}.bin` with
  fingerprint X mapped to vector V_seg
- **AND** the same directory contains a `pack-{B}.bin` with
  fingerprint X mapped to vector V_pack
- **WHEN** the reader builds its `fingerprint → vector` map
- **THEN** the resulting map contains V_pack for fingerprint X

#### Scenario: encoding more than the per-file cap fails

- **WHEN** a writer attempts to encode 511 (fingerprint, vector)
  entries into a single sidecar file
- **THEN** encoding returns an error citing the `MAX_ENTRIES` cap
- **AND** no file is written

### Requirement: Sidecar writes are atomic and lock-free

Sidecar writers SHALL produce completed files via the
write-to-tmp + rename pattern:

1. Encode the chunk bytes in memory.
2. Compute `content_hash = xxh3_64(bytes)`.
3. Write the bytes to a per-writer unique tmp path resolved via
   `Layout::writer_tmp_dir()`. The path SHALL be on the same
   filesystem as the rename destination.
4. fsync the tmp file.
5. Rename the tmp file to
   `<vectors_root>/{code|findings}/{pack|seg}-{content_hash:016x}.bin`.

If the destination already exists at step 5 (a content-identical
file from a prior write), the writer MAY skip the rename — the
existing file is byte-equal by construction.

The writer SHALL NOT acquire any advisory lock for cross-process
or cross-machine serialization. Per-writer unique tmp filenames
guarantee no collision in scratch; content-addressed destination
filenames guarantee no destructive overwrite of dissimilar
content.

`Layout::writer_tmp_dir()` SHALL resolve to
`<derived_root>/runs/{active_id}/tmp/` when `<vectors_root>` and
`<derived_root>` share a filesystem (the common case). When they
do not (e.g., `[vectors] location` points to a different mount),
it SHALL resolve to `<vectors_root>/.tmp/`. The filesystem-pair
check SHALL be performed once at `Layout::resolve()` time by
comparing device ids; the result SHALL be cached on `Layout`.

#### Scenario: two writers in the same run produce distinct tmp files

- **WHEN** two processes concurrently invoke the sidecar writer
  in the same workspace
- **THEN** each picks a distinct per-writer-unique tmp filename
  under `Layout::writer_tmp_dir()`
- **AND** neither write blocks waiting on the other
- **AND** both renames succeed (to either identical or distinct
  content-addressed destination filenames)

#### Scenario: tmp dir falls back when vectors live on a different filesystem

- **WHEN** `kenn.toml` sets `[vectors] location` to a path on a
  different mount than `<derived_root>`
- **THEN** `Layout::writer_tmp_dir()` resolves to
  `<vectors_root>/.tmp`
- **AND** sidecar renames succeed (do not return `EXDEV`)

#### Scenario: failed run sweeps its tmp directory

- **WHEN** an indexer pass crashes after writing some tmp files
  under `<derived_root>/runs/{id}/tmp/` but before completing
- **AND** the next `kenn index` invocation runs the failed-run
  cleanup
- **THEN** the entire `runs/{id}/` directory is removed,
  including its `tmp/` contents
- **AND** completed sidecar files in `<vectors_root>/...` are
  retained (renamed before the crash)

### Requirement: Snapshots carry a store-schema version that readers strictly check

`kenn-store` SHALL expose a single `STORE_SCHEMA_VERSION: u32` constant. Every published snapshot SHALL persist this value in its existing metadata blob (alongside `indexed_at`, snapshot id, and other run-level fields — no new file, no new I/O surface).

On every snapshot-open path — cold start, hot-reload after a background reindex publish, and fallback-to-parent-worktree resolution — the reader SHALL compare the persisted value to its own compiled-in `STORE_SCHEMA_VERSION` and SHALL refuse to serve the snapshot when they are not strictly equal. A snapshot whose `meta.json` exists but lacks the `schema_version` field SHALL be treated as version `1` (the pre-versioning convention). A snapshot directory with no `meta.json` at all SHALL bypass this check — the lifecycle's publish protocol refuses to mark such a run as published anyway, so the absent-meta branch never reaches a real reader on a properly-published snapshot, and the bypass keeps raw-`open_writer` test fixtures working.

A schema mismatch SHALL surface as a typed `SchemaMismatch` error from the store-open API, distinct from "snapshot not found" or "snapshot corrupt". Consumers (the MCP lifecycle, the CLI) map this error to their own user-facing form per the `mcp-orchestrated-indexing` and CLI conventions; the store itself MUST NOT mask the mismatch as a generic open failure.

#### Scenario: A snapshot written by an older binary is rejected on open

- **WHEN** a snapshot's persisted `schema_version` is `1` and the current binary's `STORE_SCHEMA_VERSION` is `2`
- **THEN** the store-open API MUST return a `SchemaMismatch` error naming both versions
- **AND** no rows from that snapshot MAY be served to any reader

#### Scenario: A snapshot written by the current binary opens normally

- **WHEN** a snapshot's persisted `schema_version` equals the current `STORE_SCHEMA_VERSION`
- **THEN** the store-open API MUST succeed
- **AND** reads MUST proceed as today

#### Scenario: A pre-versioning snapshot's meta lacks the field and is treated as version 1

- **WHEN** a snapshot's `meta.json` exists and parses but has no `schema_version` field
- **THEN** the reader MUST treat it as version `1`
- **AND** the strict-equality check MUST apply (binaries with `STORE_SCHEMA_VERSION >= 2` reject it)

#### Scenario: A snapshot directory with no meta.json bypasses the version check

- **WHEN** a snapshot directory has no `meta.json` at all (a raw `open_writer` fixture, an in-progress unpublished run, or test scaffolding that skipped the indexer's meta stamp)
- **THEN** the schema-version check MUST be bypassed — the open MUST proceed
- **AND** this bypass MUST NOT widen to any other meta-file-presence case (a present-but-malformed `meta.json`, for example, retains the v1 default)

### Requirement: Deferred runs-centric placements have direct test coverage

The deferred runs-centric layout claims SHALL hold true in code and SHALL be backed by direct tests: the indexer SHALL be exercised against per-language JSONL files written at `<derived_root>/runs/{id}/{lang}.jsonl` (not at multi-file driver-specific names); the findings store SHALL round-trip writes and reads against a Lance dataset at `<derived_root>/runs/{id}/lance/findings/` across an indexer pass; the deprecated path accessors `findings_local_dir()` and `embed_lock_path()` SHALL be removed from `Layout`; the `embed-locks/` directory SHALL no longer be created by any code path. If a workspace's `<derived_root>` already contains a stale `embed-locks/` directory from a prior layout, the indexer MAY sweep it at startup; this MUST NOT block normal operation.

#### Scenario: indexer reads JSONL from the runs-centric path

- **WHEN** an indexer pass runs against a workspace with a
  language driver that emits JSONL frames
- **THEN** the driver writes its frames to
  `<derived_root>/runs/{id}/{lang}.jsonl` (one file per language)
- **AND** the indexer ingests those frames from that path
- **AND** no `kenn-dotnet-stream-*.jsonl` or other multi-file
  driver-specific name lives outside `runs/{id}/`

#### Scenario: findings round-trip across an indexer pass via the runs-local mirror

- **GIVEN** a workspace with findings written via `kenn find`
- **WHEN** an indexer pass runs to completion and `live` is repointed
- **THEN** the new run's `lance/findings/` Lance dataset contains
  the findings
- **AND** subsequent reads through the public findings-store API
  return the same records
- **AND** no `<derived_root>/findings/` directory remains as the
  primary read path

#### Scenario: deprecated accessors and embed-locks directory are absent

- **WHEN** the workspace's `Layout` is resolved
- **THEN** the `Layout` type exposes no `findings_local_dir()` or
  `embed_lock_path()` accessor
- **AND** the indexer creates no `<derived_root>/embed-locks/`
  directory at any point during a normal pass

