## Why

`scip-indexing-pipeline` (proposal #1) defines how to *produce* normalized code structure records. We now need a place to *put* those records, a way to *swap them in* atomically when reindexing, and the surface area for a developer to *operate* the pipeline (init, index, check status, rollback). Without this, the producer has nowhere to write and consumers (the eventual query API and MCP server) have nothing to read.

The lifecycle question is non-trivial because reindexing on real workspaces takes minutes (~5 min on 1M LoC). During that window, the agent must keep answering queries against the previous, possibly stale, snapshot — never against a half-written one. The empirical anchors from the spike (91s on 303k LoC, 67 MB protobuf, 839k occurrences) tell us the steady-state DB workload: bulk inserts of ~1M rows per run, with rare full reindexes punctuated by atomic snapshot swaps.

This proposal also commits the project to a concrete embedded database. The backend ultimately adopted — a committed Lance store for hybrid search alongside an embedded redb store for the code graph — superseded the bake-off this proposal originally scoped; the live design is owned by the `index-store-db` and `indexing-orchestrator` specs.

## What Changes

- Define a per-location storage layout under `.kenn/` (with `live`, `snapshots/`, `building/`, `runs/`)
- Implement a snapshot-and-swap lifecycle: indexer writes to `building/`, on success rename to `snapshots/<timestamp>/` and atomically flip the `live` symlink
- GC policy: keep current and previous snapshot for fallback, drop everything older
- Quality-metric report on flip — comparing key counts (documents, symbols, definitions, edges, failed projects) against the previous snapshot; surface big regressions as warnings without blocking the flip
- Manual `kenn rollback` command swaps `live → previous` for bad-build recovery
- Worktree fallback: when the local `.kenn/` is absent (fresh worktree), the query layer reads the parent repo's `live` snapshot read-only; the worktree's own indexer always writes locally and never touches the parent
- Staleness signals: explicit (default), file-watcher (configurable, debounced, extension-filtered), and a git-aware skip that compares HEAD + dirty-file hashes against the previous snapshot before paying SCIP's cost
- DB choice committed: an embedded backend pairing a Lance store (hybrid search) with a redb store (code graph). The bake-off originally scoped here was superseded; the live design is owned by the `index-store-db` spec.
- Bulk-ingest implementation of the `Sink` trait from `scip-indexing-pipeline`, writing into `building/`
- CLI surface: `kenn init`, `kenn index`, `kenn status`, `kenn rollback`. The `serve` (MCP) subcommand is deferred to a later proposal.

This proposal explicitly **defers**:
- The programmatic query API (read-side operations)
- The MCP tool surface (`serve` subcommand)
- Tree-sitter Tier-1 fallback (deferred indefinitely)
- Content-addressed per-file output caching (rejected as over-engineering for SCIP's whole-solution model)

## Capabilities

### New Capabilities

- `index-store-layout`: the on-disk shape of `.kenn/` — `live` symlink, `snapshots/<timestamp>/` immutable directories, `building/` for in-progress runs, `runs/<run-id>/` for run reports — including invariants enforced across the lifecycle. Defines the contract between the producer (which writes into `building/`) and the consumer (which reads from `live`).
- `index-lifecycle`: the snapshot-and-swap state machine (steady → indexing → flip → steady'), garbage collection policy, quality-metric reporting on flip, and rollback semantics.
- `index-store-staleness`: when and why the lifecycle decides to start a new indexing run — explicit invocation, file-watcher events (debounced, filtered), git-aware skip.
- `index-store-worktree-fallback`: how a worktree without its own `.kenn/` reads the parent repo's snapshot, and the read-only invariants that protect the parent.
- `index-store-cli`: the `kenn init | index | status | rollback` developer-facing commands, their flags, and exit semantics.

The `index-store-db` and `streaming-ingestion` capabilities were originally scoped here too. Both were superseded before this change closed: `index-store-db` is now owned by its own main spec (rewritten for the Lance + redb backend), and the streaming-ingestion contract is owned by `indexing-orchestrator`. Their delta specs were dropped from this change; the four capabilities above remain its contribution.

### Modified Capabilities

(None — this is the second proposal in the system; no prior capability requirements change.)

## Impact

- **Adds the first runtime persistence layer** to the project. Before this proposal there is no DB; afterward there is.
- **First binary entry point** (`kenn`) lands. Subcommands are scoped here; `serve` lands in a later proposal but the dispatch shell exists from day one.
- **External dependency** on the embedded DB crates (`lance` for the committed hybrid-search store, `redb` for the code graph). Builds the foundation downstream proposals will not need to revisit.
- **Disk usage**: per-workspace `~220 MB × 2` for current + previous snapshot at 1M LoC. Configurable retention.
- **Filesystem invariants**: atomic rename of `building/` → `snapshots/<timestamp>` and atomic symlink flip require a POSIX-compliant filesystem. Document Windows behavior (probably MoveFileExW; out of scope for v1 if not trivial).
- **No public API yet beyond the CLI**. The query API and MCP layers are separate proposals on top of this storage.
- **Carries the producer's per-run failure tolerance forward**: if scip-indexing-pipeline's run reports `Failed`, no flip occurs; `building/` is cleaned up; previous `live` remains.
