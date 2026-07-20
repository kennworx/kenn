## Context

`Layout` (`crates/kenn-store/src/layout.rs`) is the single source of
every store path. Today it resolves three roots — `source_root`,
`committed_root = <source>/.kenn`, `derived_root` (relocatable via
`[layout] derived_root`) — and exposes accessors for every artifact:
`code_vectors_dir()`, `findings_vectors_dir()`, `live_path()`,
`snapshots_dir()`, `runs_dir()`, etc.

The sidecar (`crates/kenn-store/src/embed/sidecar.rs`) is already
content-addressed (xxh3-64 fingerprints), int8-quantized, append-log
+ compaction. The compaction-and-baseline pattern is the lever this
change moves away from.

The derived state today distinguishes:

- `local/snapshots/{ts}/` — published Lance datasets, each dataset
  as a sibling directory (knowledge, aggregate_*, analysis_*, etc.)
- `local/runs/{id}/` — indexer per-pass scratch (`report.json` today)
- `local/live` — symlink into `snapshots/`
- `local/scip-*.scip` — raw SCIP indexes, shared at `local/` root
- `local/findings/` — findings Lance, separate from snapshots

This change collapses the snapshot/run split, moves SCIP into
runs, folds every Lance dataset into a single `lance/` subdir
under the active run, **and replaces the segment+baseline sidecar
format with a content-addressed append-only pack/seg layout**.

## Decisions

### D1: Runs ≡ snapshots

There's no functional distinction between "the working directory
of an indexer pass" and "a published snapshot." The indexer writes
into `runs/{id}/` directly; on completion, `live` repoints. No
separate `snapshots/` directory exists in the new layout.

Two cleanup paths exist, kept distinct:

- **Failed-run cleanup**: a run that crashed mid-index leaves a
  partial `runs/{id}/`. On next indexer start, any non-`live`
  run with no `meta.json` (the completion stamp) is removed.
  Cheap and immediate.
- **Retention sweep**: keeps the N most recent successfully-
  completed runs (config key unchanged from today's snapshot
  retention). Triggers periodically and after each new run.

Naming: keep `runs/` (it's a noun describing what's there: the
output of an indexer run). Rejected `snapshots/` because the run
concept includes the raw inputs (SCIP/JSONL), not just the Lance
snapshot.

Run id format: ISO-8601 UTC with second-precision and no colons
(`2026-05-24T10-35-34Z`) — today's snapshot id format. Sortable
lexically (newest = max), filesystem-safe, human-scannable. The
old `run-{epoch}` format used by today's `runs/` is dropped; the
merged concept inherits the snapshot's format because that's the
agent/human-facing identifier already.

### D2: All Lance datasets under `runs/{id}/lance/`

Today's snapshot layout puts each dataset at the top level of the
snapshot directory:

```
snapshots/{ts}/
├── knowledge/                  ← lance
├── aggregate_edges/            ← lance
├── analysis_*/                 ← lance × 4
├── files/, defs/, edges/, …    ← lance × N
└── meta.json
```

New layout groups them:

```
runs/{id}/
├── tmp/                        ← scratch for atomic-rename writes (D8)
├── lance/                      ← every Lance dataset for this run
│   ├── knowledge/              ← retains the `embedding` column,
│   │                             populated from the committed sidecar;
│   │                             the ANN index is built on that column
│   ├── aggregate_*/
│   ├── analysis_*/
│   ├── files/, defs/, …
│   └── findings/               ← was: local/findings/ (separate dir)
├── *.scip                      ← was: local/scip-*.scip (shared)
├── *.jsonl                     ← per-lang JSONL frames
├── report.json                 ← indexer outcome metadata
└── meta.json                   ← snapshot stamp (timestamp, fingerprints, etc.)
```

There is no separate vectors-Lance dataset; vector queries hit
the ANN index built on `knowledge.lance.embedding`.

The `findings` Lance dataset moves from `local/findings/` (a top-
level sibling) into the per-run `lance/findings/`. Findings rows
themselves stay at `.kenn/findings/{id}.json` — only the Lance
mirror moves.

### D3: `vectors/code/` and `vectors/findings/` as siblings

Today's split — `vectors/` for code, `findings/vectors/` for
findings — is asymmetric. Both are the same artifact kind
(content-addressed embedding store). New layout puts them as
siblings under one `vectors/` parent. This makes `[vectors]
location` (D5) point at a single directory regardless of which
sidecar a caller is reaching for.

Each sidecar dir owns its own `manifest.toml` (the recipe tag
differs: `sig-lf-doc/v1` for code, `finding-text/v1` for
findings), so per-sidecar manifests stay. The `vectors/` parent
holds no manifest of its own.

Path accessors flip:

```rust
// Old
pub fn code_vectors_dir(&self) -> PathBuf {
    self.committed_root.join("vectors")
}
pub fn findings_vectors_dir(&self) -> PathBuf {
    self.committed_root.join("findings").join("vectors")
}

// New
fn vectors_root(&self) -> &Path { /* see D5 */ }
pub fn code_vectors_dir(&self) -> PathBuf {
    self.vectors_root().join("code")
}
pub fn findings_vectors_dir(&self) -> PathBuf {
    self.vectors_root().join("findings")
}
```

`vectors_root()` defaults to `<committed_root>/vectors` and is
overridable via `[vectors] location`.

### D4: SCIP files move into per-run directories

Today `local/scip-rust.scip` (and friends) live at `local/` root,
shared across all runs. This means SCIP files outlive their
generating run — handy for caching, awkward for reproducibility.

Move them per-run: `local/runs/{id}/{lang}.scip`. SCIP is cheap to
re-generate (the per-language indexer does it on demand); keeping
them per-run means each run is a complete reproducible bundle and
deleting an old run reclaims all its disk space.

If this regresses build time noticeably, add a `local/scip-cache/`
directory keyed by source-tree hash — separate change.

### D5: `[vectors] location` config

```toml
[vectors]
# Default: <committed_root>/vectors
# Override: relative path (from source_root), absolute path, or
# the keyword "global" (XDG cache, keyed by repo id).
location = "/path/to/shared/synced/vectors"
```

Value space mirrors `[layout] derived_root`. The keyword `"global"`
reuses the same xxh3-64 repo-id helper that `derived_root` uses,
so the cache namespace is shared between the two if both are set
to `"global"`.

A single location for both sidecars. No per-sidecar override
(code-only or findings-only relocation). If someone needs that
later, add `[vectors] code` and `[vectors] findings` overlays —
separate change.

Path normalization: resolve once at `Layout::resolve()`, store
the absolute `vectors_root` on `Layout`, never recompute.

### D6: On-disk format bumps to KVS2; manifest stays per-sidecar

The sidecar file format changes (see D10 for the byte layout).
Magic flips from `"KVS1"` to `"KVS2"`; `FORMAT_VERSION` bumps
from `1` to `2`.

Per-sidecar `manifest.toml` stays. Its TOML schema is unchanged
(model id, dim, quant, recipe, hash-algo); only the
`format_version` integer field bumps from `1` to `2`. The old
manifest therefore *parses* fine under the new `Manifest` struct
— the mismatch is caught one level deeper, in `load_reuse_map`'s
`m.format_version == FORMAT_VERSION` check (today at
sidecar.rs:367), which returns an empty reuse-map when the
version doesn't match. The empty reuse-map degrades reconciliation
to "embed everything." Old `.bin` files alongside the old
manifest become inert: the decoder rejects `KVS1` magic on
attempted read, and the empty reuse-map means the embed job
never tries to read them.

No auto-migration code. Workspaces on the old format: `rm -rf
.kenn && kenn index`, per the migration note in `proposal.md`.

### D7: `live` symlink behavior

`live` stays a symlink (already is today). Repoint via
`symlink(runs/{id}, live.tmp)` then `rename(live.tmp, live)` —
atomic on the same filesystem. The target is a **relative path
inside `derived_root`** (`runs/{id}`, not the absolute
`/path/to/.kenn/local/runs/{id}`) so the link stays valid if
`derived_root` is moved (`[layout] derived_root = "global"` →
XDG cache → user moves their cache).

Windows portability: not addressed in this change. If a Windows
user reports it, add a `live.txt` fallback (single-line file
containing the run id, resolved by `Layout::live_path()`).

### D8: Atomic-rename writes; tmp lives with the run, except across filesystems

The sidecar write pattern replaces today's `embed-locks/` advisory
lock with a per-writer unique tmp filename + atomic rename. Each
writer:

1. Encodes the chunk bytes in memory.
2. Computes `content_hash = xxh3_64(bytes)`.
3. Writes the bytes to a per-writer unique tmp path (see below).
4. `fsync` the tmp file.
5. `rename(tmp, vectors_root.join("{code|findings}/{pack|seg}-{hash:016x}.bin"))`.

Because every completed file is named by content hash, two
concurrent writers producing identical content rename to the same
destination — the second rename clobbers a byte-identical file,
no harm done. Two concurrent writers producing different content
write to different destinations — no collision. **No advisory lock
is needed**, on a single machine or across machines.

The tmp path needs to be on the **same filesystem** as the rename
destination (cross-fs rename returns `EXDEV`). Two cases:

- **Default — `[vectors] location` unset.** Tmp lives at
  `local/runs/{id}/tmp/{uniq}.tmp`. Same filesystem as
  `vectors/code/`. Tmp cleanup is free: failed runs are swept per
  D1, and the run's `tmp/` goes with them.

- **`[vectors] location` outside `.kenn/`.** `Layout::resolve()`
  compares the device ids of `derived_root` and `vectors_root`;
  if they differ, the tmp dir falls back to
  `{vectors_root}/.tmp/`. The vectors-co-located tmp uses a
  `.tmp` suffix that the reader skips (sync engines also
  typically skip `.tmp` files). A cold-start sweep removes
  `{vectors_root}/.tmp/*.tmp` older than one hour — bounded
  scratch debris if a writer crashed.

Because neither root directory is auto-created at
`Layout::resolve()` time, `stat()` on the root path itself may
return `ENOENT`. The device-id check SHALL therefore walk up
each path to its nearest existing ancestor and stat *that* —
device id is a mount property, so any existing ancestor on the
same mount answers the question. The result is cached on
`Layout`.

The fork is hidden in `Layout::resolve()` (one accessor:
`writer_tmp_dir() -> PathBuf`). Callers do not branch.

`vectors_root` and `derived_root` directories are not auto-created
at `Layout::resolve()` time. The first sidecar write creates
`vectors_root/{code|findings}/` via `mkdir -p` (same as today).
For `[vectors] location` pointing outside `.kenn/`, the path is
created lazily at first write — auto-creating at resolve time
could clutter unexpected directories if a user typos the path.
Permission errors on first write surface as a structured `IoError`
naming the unwritable path; the store does NOT fall back to a
different location.

### D9: Content-addressed append-only; no rewrites, no compaction, no locks

The single biggest format change: the sidecar drops the
"segment + compacted baseline" pattern entirely. Every file is
content-addressed by `xxh3_64(file_bytes)`, append-only, and
**never rewritten**. Two file prefixes split the lifecycle:

- **`pack-{content_hash}.bin`** — CI-produced, committed via git,
  the source of truth. CI is the only writer. CI's flow:
  1. Read existing `pack-*.bin` headers; build cached fp set.
  2. Walk source at this commit; compute needed fp set.
  3. Embed the diff (`needed − cached`).
  4. Write the newly-embedded fps as one new
     `pack-{content_hash}.bin` (or N packs if the diff exceeds
     the per-pack cap — see D10).
  5. Commit. **No deletion.** Old packs stay on disk.

- **`seg-{content_hash}.bin`** — dev-produced, gitignored,
  workspace-local. Same write protocol; dev's local incremental
  embeds append seg files. Reader globs both prefixes.

Three properties fall out:

1. **No multi-writer convergence problem.** Files are immutable
   once written; their names are their hashes; sync engines see
   only adds and (rarely, on `kenn gc`) deletes — never rewrites.
   No designated-writer protocol, no last-write-wins on a global
   baseline, no `sync-conflict-*.bin` surprises.

2. **No advisory locks.** Per D8, every writer's tmp filename is
   unique, and every completed filename is content-determined.

3. **No compaction code path.** `compact()` and its threshold,
   live-set filter, and segment-delete logic are deleted.
   Maintenance — bounded dead-vector cleanup when source removes
   symbols — moves to a separate **`kenn gc`** command (D11).

What "append-only" means precisely: **vector content is never
rewritten and never lost**. The directory-entry changes the
indexer is permitted to make are (a) creating a new
content-addressed file, and (b) the `--repack` seg-to-pack
rename specified in D13 (which is a directory-entry change, not
a content change — the bytes underlying the renamed file are
identical to the bytes underlying the seg-* it replaced).
Content-level deletion — actually dropping vector bytes from
the store — is reserved for `kenn gc`.

What this gives up vs. the segment+baseline format:

- Dead vectors (orphaned fps whose source symbol was deleted)
  accumulate in their original packs/segs until `kenn gc` runs.
  At dim=768 int8 + scale, ~772 B per dead entry; ~750 KB per
  1000 dead entries. Tolerable for years between gc passes.
- Pack count grows linearly with the number of CI commits that
  added any new symbols. 5000 commits → ~5000 small packs. Both
  the filesystem and git handle this — each pack is a stable
  content-addressed blob, never modified. If the directory entry
  count becomes uncomfortable, shard by first two hex chars of
  the content hash (`vectors/code/1a/pack-1a2b…bin`). Trivial
  future change.

### D10: Pack/seg file format (KVS2)

Each pack or seg file is a single content-addressed blob with a
header followed by a payload.

```
HEADER (fixed 16 B + count × 8 B, no padding)
  magic     u8[4]    "KVS2"
  ver_quant u32      low byte = quant code, upper 3 bytes = format version
  dim       u32      vector dimension
  count     u32      number of (fp, vector) entries; ≤ MAX_ENTRIES
  fp[count] u64 × count   sorted ascending, little-endian

PAYLOAD (immediately follows header)
  entry[count]: scale f32 (LE) || codes [i8; dim]
```

- **`MAX_ENTRIES = (4096 − 16) / 8 = 510`.** Writers SHALL NOT
  emit a single file with `count > MAX_ENTRIES`. The cap is
  chosen so a full file's header is exactly 4080 + 16 = 4096 B
  (one 4 KB OS page), enabling the optimization "mmap one page,
  get the whole fp list." But the cap is **a writer protocol
  constant**, not a reader assumption: readers always read
  `count` from the header and read that many fps, with no fixed
  page boundary. If a future change bumps the cap (e.g., to
  1022 to fit an 8 KB header page), the on-disk byte layout is
  unchanged and existing readers handle the larger files
  unchanged.

  The cap matters across machines because writers must agree on
  it for determinism: writer A with cap 510 and writer B with
  cap 1022 partition the same 1000-fp input set into different
  numbers of files (and therefore different content hashes /
  filenames). Bumping `MAX_ENTRIES` is a coordinated change, not
  a local tuning knob.

- **No padding.** A chunk with 5 entries is 16 + 40 = 56 B of
  header + 5 × (4 + dim) B of payload. Small files stay small.

- **`content_hash = xxh3_64(file_bytes)`**, written into the
  filename as 16-char lowercase hex. The filename verifies the
  contents — a reader may optionally check and reject files whose
  hash does not match their name (cheap integrity check).

- **Determinism.** Entries are sorted ascending by fingerprint
  before encoding. Two writers building a chunk from the same
  `(fp, vector)` set produce byte-identical files and therefore
  the same filename.

- **Batching rule** (CI's pack producer): sort the missing fp set
  ascending, take chunks of `MAX_ENTRIES`. Same fp set produces
  same chunk boundaries on every machine — incremental CI
  re-runs without source changes produce identical pack sets, no
  git churn. Dev's seg writer is unconstrained beyond the
  `MAX_ENTRIES` cap — segs are local and their hash space
  doesn't need to be stable across machines.

### D11: Pack-over-seg precedence; `kenn gc` is the only deletion path

Reader behavior on duplicate fingerprints across files:

- **Pack wins over seg.** A pack-* containing fp X overrides any
  seg-* containing fp X. Pack is CI-canonical; seg is dev-local
  and may have been computed on different hardware with ULP-level
  vector differences.
- **Within prefix, last-wins by sorted filename.** If two packs
  somehow contain the same fp (only possible after a `kenn gc`
  race that produced overlapping packs), the lexicographically
  largest filename wins. Same rule for seg-vs-seg. The vectors
  should be deterministic so this rarely matters.

Implementation: reader loads seg-* first into the map, then
pack-* (which overwrites). One line in `load_vectors`.

**Content-level deletion is reserved for `kenn gc`** — a future
maintenance command, out of scope for this change. The current
change permits exactly two directory-entry mutations: creating
a new content-addressed file, and the seg-to-pack rename during
`--repack` (D13). No code path in the indexer or embed flow
removes vector content from the store. The intended `kenn gc`
shape (recorded here for design continuity, not implemented):

```
kenn gc:
  walk source → live fp set
  load all vectors from existing pack-*/seg-*
  filter to live set
  emit new pack-* (in CI) or absorb seg-* into existing packs
  delete superseded files
```

Until `kenn gc` exists, dead-vector accumulation is the cost of
the append-only model, and is acceptable per the size analysis in
D9.

### D12: Sync-folder safety follows from D9

When `[vectors] location` points at a synced folder (Syncthing,
Dropbox, iCloud, NAS), no special-case logic is needed:

- Every committed file is immutable and content-addressed. Two
  machines producing identical content produce identical
  filenames; sync sees "same file on both sides" and accepts it.
- No file is ever rewritten in the default flow. Sync engines see
  adds (and, on `kenn gc`, deletes), never modifies.
- Two machines producing different content (because they embedded
  different fp sets, or computed slightly different vectors due
  to hardware float jitter) produce different filenames; sync
  replicates both. Reader's pack-over-seg + last-wins rule (D11)
  resolves overlaps deterministically across machines.
- The `embed-locks/` directory disappears entirely — it served
  per-machine serialization of writers to the same tmp filename,
  which the per-writer unique tmp (D8) eliminates. There was
  never a cross-machine lock, and one is still not needed.

### D13: Writer role selected by `kenn index --repack`

The pack/seg distinction is a *naming convention*, not a code
fork — both prefixes share one writer and one byte layout (D10).
The role is selected at invocation time:

- **`kenn index`** (default) — newly-embedded vectors are
  written as `seg-{hash}.bin`. Dev workflow.
- **`kenn index --repack`** — newly-embedded vectors are
  written as `pack-{hash}.bin`. Additionally, at the end of the
  run, any pre-existing `seg-{hash}.bin` in the sidecar
  directory is renamed to `pack-{hash}.bin` (prefix flip, byte
  content unchanged — the content hash, and therefore the file
  body, is identical, so the rename produces a content-equal
  pack file). CI workflow.

Properties of the `--repack` rename step:

1. **It is not a rewrite.** The bytes on disk don't change.
   `rename(seg-{hash}.bin, pack-{hash}.bin)` is a directory-
   entry change only. Atomic on POSIX.
2. **It is idempotent.** Running `kenn index --repack` against
   a workspace with no seg-* files is a no-op for the promote
   step.
3. **It does not delete data.** A seg-* file's content survives
   under the pack-* name. If the rename's target already exists
   (pack-* and seg-* with the same content hash), the seg-* is
   simply unlinked — the existing pack-* has identical bytes by
   construction.
4. **It does not bypass D9's "no rewrites" property.** The
   bytes that comprise the cached vectors never change. Only
   the directory entry that points to them changes name.

CI runs `kenn index --repack` against a fresh checkout, embeds
any missing fps, and commits the resulting `pack-*.bin` set.
Dev runs `kenn index` (no flag), which writes `seg-*.bin` that
the workspace `.gitignore` excludes from commits. There is no
auto-detection of "is this CI?" — the flag is explicit. CI
scripts add `--repack`; absence of the flag means seg-only,
period.

Running `--repack` on a dev workstation is supported but
produces commit-eligible `pack-*.bin` files (the `.gitignore`
covers `seg-*.bin` only). The flag is intended for CI by
default; devs who invoke it should understand they are
producing artifacts intended for the canonical pack set, and
should expect those files to appear in `git status` as
untracked.

If the user passes `--repack` on a workspace that contains
mixed `pack-*` and `seg-*` files (the common case: pulled
canonical packs + local segs from after the pull), the segs
are promoted into the pack namespace alongside the existing
packs. The reader's pack-over-seg precedence (D11) is
preserved trivially: after promotion, there are no segs.

## Risks

### R1: Hand-migration loses work

The "no auto-migration" decision means users who don't reset
their workspace see indexer errors when paths and format don't
match. Mitigation: clear release notes, plus the existing
graceful-degradation chain (per D6) — the old manifest parses
fine under the new `Manifest` struct; `load_reuse_map`'s
version check returns an empty reuse-map on the mismatch; the
decoder rejects `KVS1` magic if anyone tries to read an old
`.bin` file. Reconciliation degrades to re-embed rather than
crashing.

The KVS1 → KVS2 format change makes the migration heavier than
the original "just move .bin files" plan. The release notes
should now recommend the hard reset (`rm -rf .kenn`) as the
primary path; the per-file move story is obsolete (different
format, not just different parent dir).

### R2: `live` symlink atomic-rename across filesystems

If `derived_root` is on a different filesystem (`"global"` →
XDG-cache may be), the temporary symlink and the destination are
on the same filesystem (both inside `local/`), so `rename()` is
atomic. No issue.

### R3: SCIP regen cost per run

Moving SCIP per-run means each fresh run regenerates them. For
small/medium workspaces this is noise (seconds). For very large
codebases it could matter. Mitigation: deferred to a separate
`scip-cache` change if it shows up in profiling.

### R4: Dead-vector accumulation without `kenn gc`

The append-only sidecar accumulates orphaned vectors when source
symbols are deleted or renamed. Per D9, ~772 B per dead entry —
slow, bounded growth. Mitigated by `kenn gc` (future change). If
gc lands late, users with churn-heavy codebases can `rm -rf
.kenn && kenn index` to reset; cheaper than a daily prune.

### R5: Pack count growth

Linear with the number of CI commits that add any new symbols.
5000 commits ≈ 5000 small packs. Filesystem and git handle this
without issue (stable blobs, never modified). Directory listing
becomes uncomfortable around 10K+ entries; mitigated by sharding
on the first two hex chars of the content hash. Trivial future
change, not in scope here.

## Open questions

(none)
