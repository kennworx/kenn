## Why

The on-disk store layout has accumulated four issues that interact
awkwardly with the user's intended commit / sync model:

1. **Committed vectors split asymmetrically.** Today the code vector
   sidecar lives at `.kenn/vectors/` while the findings sidecar lives
   one level deeper at `.kenn/findings/vectors/`. The two are the same
   kind of artifact — content-addressed `fingerprint → vector` blobs
   — but their paths don't say so. Promoting them to siblings under a
   single `.kenn/vectors/` parent makes the "this is the synced /
   committed vector store" boundary one path, not two.

2. **Derived state is sprawled at the top of `local/`.** Today
   `local/` contains `live`, `snapshots/{ts}/`, `runs/{id}/`,
   `findings/`, `scip-rust.scip`, `embed-locks/`, `index.lock`,
   `findings.lock`, `readers/`, etc., with each Lance dataset living
   directly under a snapshot (e.g. `local/snapshots/{ts}/knowledge/`).
   Two concepts ("runs" = working state with raw inputs, "snapshots"
   = published Lance state) split what is logically one unit — the
   reproducible per-index output. Consolidating into a single
   `local/runs/{id}/` with `lance/` + `*.scip` + `*.jsonl` makes
   rollback, retention, and disk accounting one operation per
   directory.

3. **Vectors location is hard-coded.** `Layout::code_vectors_dir()`
   pins to `<committed_root>/vectors`. There's no way to point the
   committed sidecar at a sibling directory outside the repo — e.g.
   a Syncthing-managed folder shared across the team, or a peer
   cache. The xxh3-64 fingerprint content-addressing already makes
   the sidecar location-portable; only the path resolution is rigid.

4. **The segment + baseline sidecar format requires rewrites.**
   Today's sidecar maintains a single mutable `baseline.bin` plus
   append `seg-*.bin` files, with periodic compaction that rewrites
   the baseline and deletes segments. This works on one machine but
   creates real failure modes when the vectors directory is shared
   (concurrent compaction races, `sync-conflict-*.bin` files that
   the reader doesn't see), and the compaction code path is the
   only nontrivial logic in the sidecar. A purely append-only
   content-addressed format eliminates both — no file is ever
   rewritten, sync engines only see adds, and the compaction code
   path goes away.

This change reorganizes the committed vectors prefix, the derived
`local/` interior, the sidecar file format itself, and adds the
config knob to relocate the committed vectors directory.

## What Changes

### Committed structure — vectors split into `code/` and `findings/`

```
.kenn/
├── vectors/                        ← committed, sync-friendly
│   ├── code/                       ← was: .kenn/vectors/
│   │   ├── manifest.toml
│   │   ├── pack-{hash}.bin         ← CI-produced, committed
│   │   ├── pack-{hash}.bin
│   │   └── seg-{hash}.bin          ← dev-local, gitignored
│   └── findings/                   ← was: .kenn/findings/vectors/
│       ├── manifest.toml
│       ├── pack-{hash}.bin
│       └── seg-{hash}.bin
├── findings/                       ← unchanged in name; loses nested vectors/
│   └── {finding-id}.json           ← finding text + ts + hash, committed
├── local/                          ← gitignored, see below
└── .gitignore
```

The sidecar **file format** changes from segment + baseline
(`baseline.bin` + `seg-*.bin`, rewritten on compaction) to
content-addressed append-only:

- `pack-{content_hash}.bin` — CI-produced, committed via git, the
  canonical source of truth for cached vectors.
- `seg-{content_hash}.bin` — dev-produced, gitignored, local
  incremental embeds since the last `git pull`.

Both prefixes use the same byte layout: 16 B fixed header (magic
`"KVS2"`, packed version/quant, dim, count) + sorted fingerprint
list + int8 payload. A full file fits ≤ 510 entries (one 4 KB page
of header). See design D9 / D10 for the format and lifecycle.

No file is ever rewritten. CI appends new packs as source grows;
dev appends seg-* locally. The `compact()` code path goes away.
Maintenance (dead-vector cleanup) moves to a future `kenn gc`
command, out of scope for this change.

Per-sidecar `manifest.toml` stays in each of `vectors/code/` and
`vectors/findings/` — the recipe tag differs between them
(`sig-lf-doc/v1` vs `finding-text/v1`).

### Derived structure — runs-centric `local/`

```
.kenn/local/
├── live -> runs/{id}               ← relative symlink to the active run
├── runs/
│   ├── {id}/                       ← one directory per index pass
│   │   ├── tmp/                    ← scratch for atomic-rename writes
│   │   ├── *.scip                  ← per-lang SCIP indexes (raw input)
│   │   ├── *.jsonl                 ← per-lang JSONL frames (raw input)
│   │   ├── lance/                  ← every Lance dataset for this run
│   │   │   ├── knowledge/
│   │   │   ├── aggregate_edges/
│   │   │   ├── aggregate_nodes/
│   │   │   ├── analysis_*/
│   │   │   └── findings/
│   │   │                             (knowledge.lance keeps its `embedding`
│   │   │                              column — populated at index time
│   │   │                              from the committed sidecar; the ANN
│   │   │                              index is built on that column)
│   │   ├── report.json             ← indexer outcome metadata
│   │   └── meta.json               ← snapshot stamp (timestamp, fingerprints)
│   └── {old-id}/                   ← retained runs (rollback target)
├── index.lock                      ← global indexer lock
├── findings.lock                   ← findings-store lock
└── readers/                        ← reader-binding bookkeeping
```

The per-run `tmp/` directory holds in-progress vector sidecar writes
before they are renamed into `vectors/`. Failed runs sweep their
`tmp/` along with the rest of their state. The old `embed-locks/`
directory is gone — content-addressing and per-writer-unique tmp
filenames make advisory locks unnecessary (see design D8 / D9).

Two concept merges:

- **`runs/` ≡ `snapshots/`**: today's `snapshots/` directory goes
  away. Each indexer pass writes to a new `runs/{id}/`; on success,
  `live` is re-pointed to it. Old runs are retained per a retention
  policy (out of scope here — same policy as today's snapshot
  retention).
- **`scip-*.scip` moves into the run**: today they live at
  `local/scip-*.scip` (shared across runs). Moving them per-run
  makes a run a complete reproducible bundle.

The `live` symlink stays a symlink (already is today, just retargets
runs instead of snapshots). Windows portability concerns are
deferred — see Out of scope.

### Config — `[vectors] location`

```toml
[vectors]
# Optional override for the committed vectors directory.
# Default: <committed_root>/vectors
# Value can be:
#   - relative path (resolved from source_root)
#   - absolute path
#   - the keyword "global" — XDG cache path keyed by repo id
location = "/path/to/shared/synced/vectors"
```

Same value space as the existing `[layout] derived_root`. When set,
both `vectors/code/` and `vectors/findings/` move into the new
location as siblings. There is no per-sidecar override — code and
findings always live together under the chosen root.

Default location: unchanged from today. Anyone who doesn't touch
`kenn.toml` sees `.kenn/vectors/{code,findings}/`.

A future global kenn config file (not in this change, design
mentioned for forward-compat) will be able to set this default
workspace-wide.

### Migration

No automatic handling. The change ships with the new layout and a
new sidecar format (KVS2). Users on existing workspaces reset by
hand:

```
rm -rf .kenn
kenn index
```

Rationale: the format change (KVS1 → KVS2) breaks the previous
"just move .bin files" migration option. Re-embedding is the only
correct path. Sidecars and the derived `local/` are both
regenerable; auto-migration code lives in the codebase forever
for the benefit of users who don't reindex; the project is still
prototyping and there are no production users to coordinate with.

`Manifest::read` already returns `None` for incompatible
manifests, so an existing workspace will not crash on first
encountering KVS2 code — reconciliation simply degrades to
re-embed-everything, and the old KVS1 files become inert until
the user clears them.

The release notes / CHANGELOG entry for this change describes the
`rm -rf .kenn` step and notes that the previous "move files"
migration path is obsolete.

### `.kenn/.gitignore`

Old:
```
# kenn store: `local` is derived (rebuilt per worktree) — the code graph
# and the derived Lance stores. The committed data — `vectors/` and
# `findings/` — stays tracked.
local/
```

New:
```
# kenn store: derived data lives in `local/` and is rebuilt per
# worktree. The committed side holds `vectors/code/pack-*.bin`,
# `vectors/findings/pack-*.bin` (CI-produced canonical
# embeddings) and `findings/{id}.json`. The `seg-*.bin` files
# are dev-local incremental embeds — gitignored until promoted
# to `pack-*.bin` by `kenn index --repack`.
local/
vectors/code/seg-*.bin
vectors/findings/seg-*.bin
```

When `[vectors] location` points out of the repo, that path is the
user's responsibility to manage (sync tool, NAS mount, etc.); the
workspace `.gitignore` doesn't reference it.

## Capabilities

### Modified Capabilities

- **`store-layout`** — committed side: vectors split into
  `vectors/code/` and `vectors/findings/` siblings (drops the
  nested `findings/vectors/`); derived side: every index pass
  writes into `runs/{id}/` with `tmp/` + `*.scip` + `*.jsonl` +
  `lance/*`, the `snapshots/` and `embed-locks/` directories
  are removed, retention applies to runs in place. Configuration:
  `[vectors] location` relocates the committed vectors root
  independently of `[layout] derived_root`.
- **`incremental-embedding`** — sidecar **file format changes**
  (KVS1 → KVS2, segment+baseline → content-addressed
  append-only). Sidecar paths shift to `vectors/code/` and
  `vectors/findings/`. Files split into committed
  `pack-{hash}.bin` (CI-produced) and local `seg-{hash}.bin`
  (dev-produced); both share the new 16 B header + sorted-fp
  list + payload format. The `compact()` code path and
  `baseline.bin` file go away. Reader applies pack-over-seg
  precedence on duplicate fps. Advisory locks
  (`embed-locks/`) are dropped — content-addressed naming +
  per-writer unique tmp filenames + atomic rename replace them.

### Out of scope

- Retention policy *changes*. The retention sweep applies to
  runs in the new layout, and the cleanup-vs-retention split
  is specified in design D1, but the underlying "keep N most
  recent, prune older" mechanism and config key name are
  unchanged from today's snapshot retention.
- Shared / read-only vectors fallback. Out of scope per the
  "needs more research" decision earlier in the design
  conversation. Single configurable location only.
- Windows symlink fallback. Mention in passing; add only if
  reported.
- Vector quantization changes (still int8 sym pervec; only the
  containing file format changes).
- `kenn gc` — the maintenance command that prunes dead vectors
  and consolidates pack/seg files. Design D11 sketches the
  shape; implementation is a separate change. Until then,
  dead-vector accumulation is the price of the append-only model
  and is acceptable per the size analysis in D9.
- Sharding the sidecar directory by hash prefix
  (`vectors/code/1a/pack-…`). Trivial future change if file
  count grows uncomfortable.
- Global kenn config file. Schema acknowledges it; file lands
  separately.
