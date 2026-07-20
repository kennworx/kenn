## REMOVED Requirements

### Requirement: Resolved roots; only the derived root is configurable

**Reason**: Falsified by this change — `vectors_root` is now also
configurable via `[vectors] location`, so the "only derived_root is
configurable" claim no longer holds. The three-roots structure
itself is preserved and restated in the new "store separates
committed and derived data into named roots" requirement.

**Migration**: Replaced by "store separates committed and derived
data into named roots" (committed_root structure) and "Vectors
location is independently configurable" (the new vectors_root knob).

### Requirement: Every store artifact is classified committed or derived

**Reason**: Replaced by the more detailed enumeration in the new
"store separates committed and derived data into named roots"
requirement, which lists what each root holds under the runs-centric
layout (no more `snapshots/`, vectors split into `code/` and
`findings/`).

**Migration**: Replaced by "store separates committed and derived
data into named roots".

### Requirement: The derived root may be relocated, including globally

**Reason**: Folded into the new "store separates committed and
derived data into named roots" requirement, which restates
`derived_root` relocation as part of its three-roots structure.
The XDG-keyword resolution and staleness-key gating behaviors are
preserved unchanged; only the spec organization changes.

**Migration**: Replaced by "store separates committed and derived
data into named roots".

### Requirement: Snapshot selected by staleness key

**Reason**: The store no longer maintains a separate `snapshots/`
directory — each `kenn index` pass writes a `runs/{id}/` directly,
and `live` points at the active run. The staleness-key mechanism
itself is preserved (recorded in `meta.json` per run) but applies
to runs, not snapshots. Spec text would have been rewritten with
"snapshot" → "run" throughout, which is more honest as a removal +
re-statement than a modification.

**Migration**: Run selection by staleness key is described in the
new "Derived state is runs-centric; no separate snapshots directory"
requirement (which states that rollback re-points `live` to a prior
`runs/{id}/`) and is exercised by `decide_startup_state` in
`kenn-store/src/lifecycle.rs`.

### Requirement: Snapshot retention is bounded by recent use

**Reason**: Same removal trigger as the staleness-key requirement —
no `snapshots/` directory under the runs-centric layout. The
LRU-based retention policy is preserved (it now applies to
`runs/{id}/` entries) and the live-exemption and reader-held
exemptions still hold, but the spec text needed to be re-written
end-to-end with "snapshot" → "run".

**Migration**: Run retention is part of the indexing lifecycle and
implemented by `gc()` in `kenn-store/src/lifecycle.rs`. The
existing tests (`gc_keeps_n_and_drops_the_rest`,
`actively_used_branch_snapshot_survives_another_branch_reindex`)
exercise the LRU policy against runs.

## ADDED Requirements

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

- `findings/{id}.json` — finding records (source of truth).
- `vectors/code/` — committed code embedding sidecar.
- `vectors/findings/` — committed findings embedding sidecar.
- `.gitignore` — excludes `local/`.

The derived root holds:

- `runs/{id}/` — one directory per index pass (see "runs-centric
  derived state" requirement).
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

### Requirement: Vectors location is independently configurable

The committed vectors root SHALL be relocatable via the
`[vectors] location` config setting, which accepts the same value
space as `[layout] derived_root`: a relative path (resolved from
`source_root`), an absolute path, or the keyword `"global"`
(resolved to an XDG cache path keyed by the repo id).

When set, both `code_vectors_dir()` and `findings_vectors_dir()`
SHALL resolve under the new location as siblings (`<location>/code/`
and `<location>/findings/`). There is no per-sidecar override.

The default — when `[vectors] location` is unset — SHALL be
`<committed_root>/vectors`, preserving today's location semantics.

#### Scenario: vectors location override moves both sidecars

- **WHEN** `kenn.toml` sets `[vectors] location =
  "/mnt/shared/kenn-vectors"`
- **THEN** `vectors_root` resolves to `/mnt/shared/kenn-vectors`
- **AND** `code_vectors_dir()` resolves to
  `/mnt/shared/kenn-vectors/code`
- **AND** `findings_vectors_dir()` resolves to
  `/mnt/shared/kenn-vectors/findings`
- **AND** `committed_root` still resolves to `<source>/.kenn`,
  unchanged

#### Scenario: vectors location accepts the "global" keyword

- **WHEN** `kenn.toml` sets `[vectors] location = "global"`
- **THEN** `vectors_root` resolves to an XDG cache path keyed by
  the same repo id used by `[layout] derived_root = "global"`
- **AND** the path is stable across `Layout::resolve` calls for
  the same workspace

#### Scenario: vectors_root is created lazily on first write

- **WHEN** `Layout::resolve()` runs against a workspace whose
  `[vectors] location` points at a directory that does not exist
- **THEN** `Layout::resolve()` succeeds without creating the
  directory
- **AND** the directory is created (`mkdir -p`) by the sidecar
  on the first vector write
- **AND** if the user has no write permission on the parent
  path, the first vector write returns an IO error naming the
  unwritable path, not a silent fallback to a different location

### Requirement: Derived state is runs-centric; no separate snapshots directory

Every index pass SHALL write its output into a single directory
`<derived_root>/runs/{id}/` containing the raw inputs, all Lance
datasets, and per-run metadata. On successful completion, the
`<derived_root>/live` symlink is repointed to that run.

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
Rollback re-points the `live` symlink to a prior `runs/{id}/`. The
`live` symlink target SHALL be a relative path
(`runs/{id}`, not an absolute path) so that the link remains valid
when `derived_root` is moved on disk.

#### Scenario: a successful index pass creates one runs directory

- **WHEN** `kenn index` runs against a workspace
- **THEN** a directory `<derived_root>/runs/{new_id}/` is created
- **AND** every Lance dataset for the run lives under
  `<derived_root>/runs/{new_id}/lance/`
- **AND** per-language `.scip` and `.jsonl` files live at
  `<derived_root>/runs/{new_id}/`
- **AND** the `<derived_root>/live` symlink target is
  `runs/{new_id}` (relative path, no trailing slash)
- **AND** no `<derived_root>/snapshots/` directory exists

#### Scenario: rollback repoints live to a prior run

- **WHEN** the workspace has runs A (older) and B (active),
  with `live -> runs/B`
- **AND** the user invokes `kenn rollback`
- **THEN** `live` is atomically repointed to `runs/A`
- **AND** `runs/B/` remains on disk until the retention sweep
  removes it

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
