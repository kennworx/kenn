# `.kenn/` store architecture

Implementation reference for kenn's storage and lifecycle layer
(`crates/kenn-store`). It complements `crates/kenn-cli/README.md`
(user-facing) with the why behind the design and the diagrams that make
the lifecycle make sense. The crate implements the `index-store-layout`,
`index-lifecycle`, `index-store-staleness`,
`index-store-worktree-fallback`, and `storage-backend-abstraction`
capabilities.

## Storage layout

`.kenn/` splits into a **committed** subtree (git-tracked) and a
**derived** subtree (`local/`, gitignored). A *run* is a snapshot: each
`kenn index` pass writes a new run directory, and `live` is flipped to it
atomically on success.

```
<workspace>/
└── .kenn/                                    committed_root
    ├── .gitignore                            (ignores local/)
    ├── local/                                derived_root — gitignored
    │   ├── live  -> runs/<timestamp>/        relative symlink to current run
    │   ├── runs/
    │   │   ├── 2026-05-01T12-30-00Z/         previous (rollback target)
    │   │   └── 2026-05-01T15-45-00Z/         current (`live` points here)
    │   │       ├── graph.db                  SQLite: code graph
    │   │       ├── knowledge.db              SQLite: search (FTS5) + vectors (vec0)
    │   │       ├── meta.json                 status, schema_version, backend marker
    │   │       ├── <lang>.scip               indexer input
    │   │       ├── <lang>.jsonl              indexer input
    │   │       └── tmp/                       per-run atomic-rename staging
    │   ├── index.lock                        exclusive flock (single writer)
    │   ├── readers/                           reader GC pins
    │   └── findings-publish.lock
    ├── vectors/                              committed sidecar (git-tracked)
    │   ├── code/                              fingerprint -> embedding (packs/segments)
    │   └── findings/
    └── findings/                             durable findings store
```

Invariants:
- A run directory is **published** once `meta.json` is written into it;
  only published runs are eligible to be the `live` target or to be kept
  by GC. A run **without** `meta.json` is incomplete (mid-pass or crashed)
  and is swept by `recover`.
- A published run is **immutable**. The only legal operation on it is
  recursive deletion during GC.
- `live` is a **relative** symlink under `local/`, so the tree is
  relocatable.
- There is no separate `building/` directory: the run dir *is* the
  output dir, and `tmp/` inside it stages files for atomic rename. This
  is the §D1 invariant — "the run directory IS the published directory."

## Lifecycle state machine

```
                    ┌──────────────────┐
                    │  Uninitialized   │
                    │  (no `live`)     │
                    └────────┬─────────┘
                             │ first `kenn index`
                             ▼
   ┌─────────────────────────────────────────────────────────┐
   │   Steady(T_n)  — `live -> local/runs/T_n/`               │
   └────────────────────────┬────────────────────────────────┘
                            │ begin_indexing (flock + mkdir runs/T_{n+1}/)
                            ▼
   ┌─────────────────────────────────────────────────────────┐
   │   Indexing(T_{n+1})                                      │
   │   - readers continue against T_n (live unchanged)        │
   │   - BatchSink writes directly into runs/T_{n+1}/         │
   └──────┬───────────────────┬──────────────────────────────┘
          │                   │
          │ abort/no-meta     │ publish
          ▼                   ▼
   ┌──────────────┐    ┌──────────────────────────────────────┐
   │  Discarded   │    │  Publish:                            │
   │  - rm -rf    │    │  1. write meta.json into the run     │
   │    runs/     │    │  2. flip `live` via tmp-symlink      │
   │    T_{n+1}/  │    │     + rename(2)                       │
   │  - live      │    └────────────────┬─────────────────────┘
   │    unchanged │                     │
   └──────┬───────┘                     ▼
          │                       Steady(T_{n+1})
          └────► Steady(T_n)      + GC runs older than the retained set
```

A handle dropped without `publish()` or `abort()` (e.g. a panic) is judged
by `meta.json` presence: a run **without** `meta.json` is incomplete and
`recover` deletes it on the next indexer start; a run **with** `meta.json`
(a `publish()` that stamped meta but then failed on fsync or the symlink
flip) is **kept**, since its data is complete — losing it would discard a
full index over a failed `live` flip.

## Atomic publish (POSIX)

The run directory is never renamed — it is written in place and stamped
with `meta.json`. Only the `live` symlink moves:

```
   .kenn/local/
   ├── live           -> runs/T_n/             (existing)
   ├── live.tmp.<pid> -> runs/T_{n+1}/         (new symlink, just created)
   └── runs/
       ├── T_n/                                 (current)
       └── T_{n+1}/                             (built + meta.json stamped)

       rename(2): `live.tmp.<pid>` → `live`
       Atomic on POSIX. Readers that resolved `live` *before* this point
       keep their inode handle and continue reading T_n; readers that
       resolve `live` *after* see T_{n+1}.
```

Windows is a v1 gap; the eventual path is `MoveFileExW` over a junction.

## Worktree → parent fallback

```
   /repo/                            (main worktree)
   └── .kenn/local/live -> runs/A/

   /elsewhere/feature-x/             (linked worktree, no .kenn/)

   open_for_read(/elsewhere/feature-x)
        │
        ▼
   ┌─────────────────────────────────────────────────────────┐
   │ 1. Local `.kenn/local/live` ?     No.                    │
   │ 2. `git worktree list --porcelain` first record          │
   │    -> /repo (main worktree)                              │
   │ 3. /repo/.kenn/local/live exists -> open read-only       │
   │ 4. Tag the read context as `FallbackFromParent`          │
   │    so consumers can label results.                       │
   └─────────────────────────────────────────────────────────┘
```

The worktree's own indexer **never** writes to the parent's `.kenn/`: no
lock, no run dir, no GC. The parent is a passive read source
(`crates/kenn-store/src/worktree.rs`).

## Storage backend: SQLite

There is a **single** storage engine — SQLite — selected unconditionally
(no cargo features). SurrealDB/RocksDB and redb/tantivy are gone; so are
Lance/DataFusion/Arrow. `meta.json` carries `backend: "sqlite"`
(`crates/kenn-store/src/meta.rs`), and `open_reader` refuses a snapshot
stamped with a different `STORE_SCHEMA_VERSION`.

A published run holds two SQLite databases:

- **`graph.db`** — the code graph: `symbols`, `symbol_docs`, `file_docs`,
  `defs`, `edges`, `files`, `packages`, and the `aggregate_*` /
  `analysis_*` projections.
- **`knowledge.db`** — the search store: a `knowledge` table, two **FTS5**
  virtual tables (trigram + porter tokenizers), and a **`vec0`** virtual
  table (`sqlite-vec`) for vector KNN.

The schema is canonical SQLite DDL held as Rust string constants
(`GRAPH_DDL`, `KNOWLEDGE_DDL`) in
`crates/kenn-store/src/db/sqlite/schema.rs`, applied to each fresh run.
The storage trait surface (`api::Reader`, `api::WriteBatch`) is reached
only through `open_writer` / `open_reader`; the concrete backend lives in
the private `db` module.

## Ingest pipeline

Each language ingester owns a `BatchSink`
(`crates/kenn-indexer/src/sink.rs`). The sink accumulates parsed records
into a `WriteBatch`; when the batch fills (`config.ingest.batch_size`,
default 10_000) it appends via `writer.write_batch(&batch)`, which the
SQLite writer commits as **one transaction per batch**
(`crates/kenn-store/src/db/sqlite/writer/core.rs`). The ingester runs on a
plain OS thread, so the sink drives the async append with
`Handle::block_on`.

Memory is bounded: peak RSS is `O(batch_size × max_record_size + 1
document)`. At finalize, committed vectors are reconciled from the
sidecar into the run's `vec0` table, and FTS5 indexes are built before
`meta.json` is stamped.

## Vectors: committed sidecar

Embeddings are **not** rebuilt on every index. They live in a committed,
git-tracked sidecar at `.kenn/vectors/{code,findings}/`, keyed by content
fingerprint (`crates/kenn-store/src/embed/sidecar/`):

- `pack-<hash>.bin` — immutable, CI-produced, **committed** packs.
- `seg-<hash>.bin` — dev-local incremental segments (**gitignored**;
  promoted to a pack via a byte-preserving rename).
- `manifest.toml` — embedding-model metadata.

The sidecar is content-addressed, so an unchanged symbol keeps its vector
across reindexes; only new/changed fingerprints are embedded. At publish,
the relevant vectors are loaded into the run's `vec0` table for KNN
queries. The sidecar location is relocatable via `[vectors] location` in
`kenn.toml`.

## Failure semantics

| Trigger | What happens |
|---|---|
| One indexer subprocess fails (e.g. `kenn-dotnet` exits non-zero) | Its per-unit `RunReport` is `Failed`; the pipeline continues with other units |
| **Every** unit reports `Failed` | `cmd_index` aborts the run (deletes `runs/T_{n+1}/`) and exits non-zero (`ExitCodes::IndexerFailed`); `live` unchanged |
| `Partial` aggregate (some units ok, some failed) | The run still publishes — only an **all-`Failed`** aggregate aborts |
| Process killed mid-pass (before `meta.json` is written) | The run has no `meta.json`; the next indexer start's `recover` deletes the incomplete run. `live` is unchanged |
| Process killed mid-publish (after `meta.json`, before the symlink flip) | The run is complete and is **kept** (not deleted by `recover` — it has `meta.json`); `live` still points at the prior run. As a published-but-not-`live` run it is an ordinary LRU eviction candidate, reclaimed by a later `gc` once it ages past `retention` (unless reader-pinned) |
| Second `kenn index` while the lock is held | `begin_indexing` fails with `LockHeld`; the invocation exits non-zero with a clear message (`ExitCodes::LockHeld`), touching nothing |

## Concurrency model

Single writer per workspace, enforced by an exclusive `flock(2)` on
`.kenn/local/index.lock` (acquired non-blocking by `begin_indexing`).
Readers do not lock — published-run immutability plus
opened-file-handle semantics handle read-during-flip correctness. Active
readers pin runs against GC via `local/readers/`.

## Why xxhash for the staleness key

We need fast (sub-100 ms on typical edit cycles) deterministic identity
checks on dirty file contents. We do **not** need cryptographic strength:
the staleness key is never used as content-addressed storage — it's a
"did anything change since the last run" signal. xxh64/xxh3 is the right
trade-off; sha256 would be overkill
(`crates/kenn-store/src/staleness.rs`).

## What this layer does NOT do

- **Defining table shapes for new edge/symbol kinds**: that belongs in the
  source data model; this layer persists the records the producer emits
  and serves them back read-only.
- **Background scheduler / file-watcher**: opt-in, off by default
  (`kenn.toml [staleness] file_watcher = false`).
- **Cross-machine cache**: out of scope.

The query/read API itself is **not** here — it lives in `kenn-mcp`
(`crates/kenn-mcp/src/tools/`: `query`, `semantic`, `findings`,
`lifecycle`, `state`), which opens this layer read-only.
