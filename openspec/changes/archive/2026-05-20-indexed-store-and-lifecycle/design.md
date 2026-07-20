## Context

`scip-indexing-pipeline` produces normalized records (symbols, occurrences, edges) into a `Sink`. This proposal defines where those records land, how reads are served while writes are in flight, how reindexing is triggered, and how the user operates the system from a CLI.

The **logical schema** (table shapes, relations, public ID format, indexes, kind enum, wire location format) is defined in the `source-data-model` proposal. This proposal references that schema; it is the storage and lifecycle layer beneath it. Changes to the schema (adding a relation kind, changing a column) belong in `source-data-model`; changes to atomicity, snapshot layout, or ingest flow belong here.

Empirical anchors from the spike:
- a 303k-LoC C# spike: SCIP run is 91 s, output 67 MB protobuf, 839k occurrences, 122k defs, 379 explicit relationship edges.
- Linear projection to 1M LoC: ~5 min, ~220 MB.
- Implication: re-indexing on every change is *not* viable. Reads must continue against the old snapshot for minutes while a new one builds.

User constraints set in earlier discussion:
- Local developer machines, single-developer-at-a-time activity per workspace.
- Branch switches and remote-fetched commits are the dominant invalidation events.
- Worktrees are first-class — a worktree should not have to wait for its own index before being usable.
- Tree-sitter Tier-1 fallback is deferred indefinitely; this design assumes SCIP-or-nothing.
- Per-file content-addressed caching was explicitly rejected as over-engineering for SCIP's whole-solution model.

## Goals / Non-Goals

**Goals:**
- A `.kenn/` storage layout with snapshot directories and an atomic-flip `live` symlink
- A lifecycle that lets readers query stale data uninterrupted while a new index builds
- GC retaining current + previous, with `kenn rollback` to swap them
- Quality-metric report on every flip, surfacing regressions without blocking
- Worktree → parent fallback for read traffic, with strict no-write-to-parent
- Staleness signals: explicit (default), file-watcher (optional, debounced, filtered), git-aware skip
- A committed embedded DB choice (Lance for hybrid search + redb for the code graph)
- A `kenn` CLI with `init | index | status | rollback`
- Implementation grounded in real numbers from the spike workload

**Non-Goals:**
- Programmatic query API surface (separate proposal)
- MCP `serve` subcommand (separate proposal)
- Tree-sitter fallback (deferred indefinitely)
- Content-addressed per-file caching
- Cross-machine cache (purely local, per-machine)
- Windows support in v1 (POSIX symlink and atomic-rename are required); document as a known gap
- Multi-process concurrent writers; a single workspace has a single writer at any time enforced by a lock file

## Decisions

### D1. Storage layout: `.kenn/{live, snapshots/, building/, runs/}`

Per `index-store-layout` spec. Snapshots are immutable directories named by ISO-8601 UTC timestamp. `live` is a relative symlink into `snapshots/`. `building/` exists only during a run. `runs/<run-id>/report.json` persists run metadata independently of snapshot lifetime.

Rationale: keeps reader and writer disjoint at the filesystem level. No DB-level concurrency tricks needed; OS guarantees do the work.

Alternatives considered:
- Single mutable DB with transactions for swaps. Rejected — transaction overhead for replacing ~1 M rows is huge, and bulk-replace doesn't fit transactional semantics cleanly.
- Versioned tables inside one DB ("`symbols_v17`"). Rejected — couples versioning to schema, hard to GC cleanly, awkward read path.

### D2. Atomic flip via `rename(2)` of a symlink

POSIX `rename(2)` is atomic on the same filesystem. The flip sequence:
1. Create a fresh tmp symlink in `.kenn/` pointing at the new snapshot (e.g., `.kenn/live.tmp.<pid>`)
2. `rename(.kenn/live.tmp.<pid>, .kenn/live)` — atomic replacement

Readers that resolved `live` before step 2 retain their open file handles; the OS keeps the inodes alive even after the symlink target changes.

Windows is out of scope for v1. The eventual Windows path is `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` over a directory junction — feasible but non-trivial to integrate with the rest of the design.

### D3. Failed-run isolation: never publish on Failed

Per `index-lifecycle`. If the run reports `Failed` or the process dies, `building/` is deleted on next startup; `live` is unchanged. We never publish a partial snapshot.

`Partial` runs (the partial-failure pattern, where some projects fail but data is produced) DO publish. The post-flip metric report surfaces what was missed.

### D4. GC: keep current + previous

Per `index-lifecycle`. Two retention slots cover the rollback case. Older snapshots are deleted on a background task scheduled immediately after a successful flip. Deletion uses `rm -rf` semantics; no file in a still-retained snapshot can be touched.

GC interruption: a process crash mid-deletion leaves orphan files. On next startup, any directory under `snapshots/` that is neither the live target nor the previous one is scheduled for deletion.

### D5. Quality metrics on flip — observability, never gating

Per `index-lifecycle`. The metric report compares new vs previous and identifies regressions over a configurable threshold (default 10 %). Warnings are persisted in the new run's report and surfaced via `kenn status`. The flip is *never* blocked, because blocking on bad metrics would leave agents stranded on stale data with no automatic recovery path.

User recourse for a bad build: `kenn rollback`.

### D6. Worktree fallback: read parent's `live` read-only

Per `index-store-worktree-fallback`. Discovery uses `git worktree list --porcelain` (or `gix` equivalent) to identify the main worktree path. Fallback opens that path's `live` in DB-specific read-only mode.

Strict invariant: a worktree never writes to the parent. No locks, no schema migrations, nothing. The parent is a passive read source.

If the worktree later gets its own `live`, queries flip to local on next read open. We do not "promote" the parent's data into the worktree's index — the worktree's own indexer builds from scratch.

### D7. Staleness signals: explicit + git-aware skip + optional file-watcher

`kenn index` (default) checks the git-aware staleness key: `(HEAD commit, sorted [(dirty_path, xxhash)])`. If the key matches the current snapshot's recorded key, skip the run. Otherwise, run the indexer.

`--force` bypasses the skip.

File-watcher is opt-in via config. When enabled: notify-rs watches the workspace, applies extension and exclude filters, debounces 30 s of inactivity, then triggers an `index` invocation (which still goes through the staleness check). Default file-watcher is OFF to avoid surprising new users.

Rationale: developers on local machines hit branch-switch and edit-cycles as the dominant change events. Git-aware skip handles the "switched branch but no edits" case essentially free. File-watcher handles "I edit files all day" but introduces noise risk; opt-in is conservative.

### D8. DB choice: Lance (hybrid search) + redb (code graph)

Per `index-store-db`. This decision originally proposed an empirical bake-off between SurrealDB embedded (RocksDB) and a SQLite + tantivy + petgraph stack, run from a `crates/db-bakeoff/` sub-crate. That bake-off was superseded: the project first adopted SurrealDB embedded, then — for BM25 ranking quality and operational simplicity — migrated to the backend that ships today.

The live backend is two embedded stores behind one `kenn-store` API:

- **Lance** — a committed, durable columnar store carrying the symbol and doc tables plus the hybrid (BM25 + n-gram) search indexes. Lives at `.kenn/knowledge/` and is checked into the repo.
- **redb** — a per-branch embedded key-value store carrying the code graph (edges, traversal indexes). Lives under `.kenn/local/snapshots/<timestamp>/` and is gitignored.

The schema mapping from `kenn-model` records to store rows, and the rationale for the two-store split, are owned by the `index-store-db` and `storage-backend-abstraction` specs.

### D9. Streaming pipeline architecture

Per the original `streaming-ingestion` capability. **This capability was superseded by `indexing-orchestrator`**, which specs the live design — a four-phase run (prepare → ingest → aggregate → finalize) streaming records to a single DB-writer over a bounded channel, with the atomic both-stores publish at finalize. The pipeline as originally designed:

```
   producer thread                       consumer thread
   ───────────────                       ───────────────
   parse Document i                       pull batch of N
        │                                       │
   transform → records                          ▼
        │                                  Phase 1: bulk insert
        ▼                                  with deferred indexes
   Sink::write_*  ─── bounded ───►        (no FTS update,
                      channel              no secondary indexes)
                      size B                      │
                                                  │ on end_run(Success):
                                                  ▼
                                             flush remaining batch
                                                  │
                                                  ▼
                                             Phase 2: build indexes
                                                  │
                                                  ▼
                                             fsync, return
```

Key invariants:
- **Memory bounded by `O(B × max_record_size + 1 document)`** — independent of total record count. Validated by load test on the the C# spike workload.
- **Back-pressure is the channel filling up.** The producer's next `write_*` blocks. No spillover, no drop, no second buffer.
- **Phase 1 minimizes overhead.** Deferred indexes / disabled FTS commits / `PRAGMA synchronous=OFF` (with the journal mode chosen to still recover safely; we accept that an unflushed Phase 1 buffer is *lost* on crash because we'll re-run from scratch). Index build happens in Phase 2 with the engine's native bulk-load primitives.
- **Phase 2 happens before publish.** The lifecycle's atomic flip is gated on `Sink::end_run(Success)` returning, which only happens after Phase 2 + fsync.
- **Error propagation:** consumer-side write errors flow back through `Sink::write_*`'s return value; producer-side failures call `end_run(Failed)` which skips Phase 2.
- **Concurrency model:** producer thread + consumer thread + one bounded channel (default size determined by the bake-off; the `tokio::sync::mpsc` or `crossbeam_channel` backend is implementation-defined). A degenerate `B = 0` synchronous mode is supported for tests and for the embedded sync use case.

Alternatives considered:
- **Single-phase ingest.** Reject — query-realistic schemas have indexes that triple Phase-1 cost when updated synchronously. The two-phase pattern is well-trodden (Postgres COPY + later CREATE INDEX, SQLite deferred index creation, Lucene/tantivy segment-then-merge).
- **Synchronous iteration without a channel.** Acceptable for small workloads but wastes parallelism; on the spike the producer is faster than the consumer. Threaded version isn't much harder; we ship threaded.
- **Disk-backed spillover queue.** Reject — adds a layer that buys nothing for our bounded-memory design; back-pressure is the simpler solution.

### D9b. DB Sink concrete behavior

Per `index-store-db` and `streaming-ingestion`:

- **`begin_run(report)`**: take exclusive lock on `.kenn/index.lock`, `mkdir building/`, open fresh DB inside `building/`, apply schema migration with index creation suspended where supported.
- **`write_symbol|occurrence|edge`**: append to in-memory batch; when batch reaches the configured size threshold or time threshold, flush to DB inside Phase 1 (raw insert).
- **`end_run(Success)`**: drain channel, flush final batch, run Phase 2 (build indexes / commit FTS segments / serialize derived structures), `fsync`, release lock, return.
- **`end_run(Failed)`**: close DB handles cleanly without Phase 2, release lock, return. Lifecycle deletes `building/`.

Batch size, channel size, and time thresholds are configuration knobs whose defaults are tuned empirically.

### D10. CLI: subcommand dispatch, no MCP yet

Per `index-store-cli`. `kenn <subcommand>` shell using `clap`. The four subcommands defined here are scoped tightly. The future `serve` subcommand will be added by the MCP-server proposal — no skeleton today, no placeholder. The CLI's `--help` lists the four-and-only-four available now; we will not ship "coming soon" stubs.

### D11. Concurrency model: one writer, many readers

Per `index-lifecycle`. An exclusive flock on `.kenn/index.lock` guards writes. Reads do not lock. Snapshot immutability + opened-file-handle semantics make read-side concurrency a non-event.

Implementation: `fs2::FileExt::try_lock_exclusive` or `nix::fcntl::flock` for cross-platform-ish flock. Document the lock file path so users can debug stuck locks.

### D12. Git-aware skip uses xxhash, not sha256

For the dirty-file content hash, `xxhash` is fast enough that hashing a few dozen typically-edited files is sub-100 ms. We do not need cryptographic strength for staleness detection — we need speed. xxhash is a stable identity function for "did this file change since last run?"

(`sha256` would be needed if hashes were used as identity in a content-addressed store. Since we rejected content-addressing, xxhash suffices.)

### D13. Configuration: `kenn.toml`, sane defaults

Schema (initial):

```
[workspace]
root = "."     # default: discovered via git toplevel

[language.csharp]
enabled = true                            # auto-detected from .sln presence
provision_directory_build_props = false   # opt-in from scip-indexing-pipeline

[exclude]
globs = ["node_modules/", "bin/", "obj/", "target/"]

[lifecycle]
gc_keep = 2

[staleness]
git_aware_skip = true
file_watcher = false
file_watcher_debounce_ms = 30000
file_watcher_extensions = [".cs", ".csproj", ".sln", ".props", ".targets"]

[metrics]
regression_threshold_pct = 10
```

Most users should not need to touch this. `kenn init` writes a populated copy.

## Risks / Trade-offs

- **[Risk] Atomic-symlink rename is POSIX-only.** Windows users are blocked until we add a junction-based path. → Mitigation: documented as a v1 limitation; design leaves a clear extension point in the lifecycle module.
- **[Risk] DB choice locks us in for a long time.** Migrations are painful. → Mitigation: the bake-off methodology is itself reproducible; if six months later the winner has aged badly, we re-run the bake-off and migrate. The data model is DB-independent so a port is bounded work.
- **[Risk] File-watcher noise floods the queue.** → Mitigation: opt-in default off; aggressive filters; 30 s debounce. Even when on, the git-aware skip is the final gate before paying SCIP's cost.
- **[Risk] Branch switch with rebase rewrites HEAD many times in seconds.** → Mitigation: lock file means subsequent triggers wait; debounce collapses bursts; staleness check at the bottom decides if any work is actually due.
- **[Risk] Quality-metric thresholds fire too often (false positives) or not often enough (real regressions slip).** → Mitigation: tune in the field; thresholds are configurable; the report carries raw numbers so users can audit.
- **[Risk] Worktree fallback to a stale parent is misleading.** Agent reads parent data thinking it's worktree data. → Mitigation: read context explicitly tagged "fallback from parent"; consumers (and eventually the MCP) MUST surface this state. Specced.
- **[Risk] GC of an in-use snapshot.** Reader has the inode open; GC unlinks the path. POSIX keeps the inode alive but disk space stays used until the reader closes. → Acceptable: brief disk-pressure window during long-lived reader rare; documented.
- **[Trade-off] No AST-based Tier-1 means a fresh clone with no SCIP toolchain returns "unavailable" for code-intel.** → Acceptable per current scope; an AST-based fallback is parked for later if it becomes painful.
- **[Trade-off] Single binary instead of library + thin CLI.** → Day-1 simpler. Library extraction can happen later if a separate consumer needs it.

## Migration Plan

Greenfield. No migration. First proposal that ships actual code.

`kenn init` is the user's first command after `cargo install` (or equivalent).

## Open Questions

- **Bake-off reporting format.** Markdown table is fine. Do we also want an automated assertion gate (fail-the-build if the winner regresses)? Probably not for v1; track manually.
- **Parent-fallback when parent is currently indexing.** If the parent's indexer is running, parent's `live` points at its previous snapshot — the worktree reads stable data. But: should the worktree be told "parent is currently building, expect a fresher snapshot soon"? Probably yes; can be added to `kenn status` cheaply.
- **xxhash variant.** XXH3 (latest) vs XXH64 (older, more libraries). Use whatever the chosen Rust crate defaults to; document.
- **File-watcher cross-platform.** `notify-rs` is the obvious choice; does its API differ enough on macOS FSEvents vs Linux inotify to bite us? Spike with a small fixture during impl.
- **Where are run reports stored long term?** Spec says retain 30 days regardless of snapshot lifetime. Concrete cleanup mechanism (cron-like? on each `kenn index` invocation?) → Decide during impl.
- **Disk-space safety net.** Before `kenn index` starts, should we precheck for free space (e.g., 2× expected snapshot size)? Probably yes; punt to an enhancement task.
