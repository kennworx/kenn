## Why

`vector-store-layout` shipped the runs-centric structure, the
configurable vectors location, and the KVS2 sidecar format. Five
small cleanups were carved out because they reach across module
boundaries the parent change didn't want to widen:

| Deferred task | What it actually wants |
|---|---|
| 5.4 JSONL stream files into the run | path-construction change in `make_scratch_stream_path` |
| 5.5 Findings Lance into `runs/{id}/lance/findings/` | wire findings rebuild through the indexer pass and read through `live` (same lifecycle as code-graph Lance) |
| 3.9 Move `embed.lock` into the run | relocate the per-snapshot flock from `derived_root/embed-locks/<id>` to `runs/{id}/embed.lock` so the lock lives with what it protects |
| 1.11 Remove `findings_local_dir()` | unblocked by 5.5 |
| 1.13 Remove `embed_lock_path` (old shape) | unblocked by 3.9; the accessor's new shape is per-run, not under `embed-locks/` |

The `store-layout` spec already says runs/{id}/ contains the
per-language JSONL and the findings Lance mirror, and that
`embed-locks/` is gone — closing that gap is this change's job.
None of the three blocks needs an external design call. The lock
that protects the embed pass is **kept** — Lance writes during
NULL-embedding fill need cross-process dedup — only its filesystem
location changes.

## What Changes

- **JSONL stream files move into the run** (5.4). In
  `crates/kenn-indexer/src/driver.rs`, `make_scratch_stream_path`
  switches from `derived_root/kenn-dotnet-stream-{pid}-{n}.jsonl`
  to `runs/{id}/kenn-dotnet-stream-{pid}-{n}.jsonl`. The
  pid+counter naming is **kept** — it handles retry/concurrency
  within a single run. Caller plumbing carries `run_id` (or the
  run dir path) into the driver-invocation path.
- **Findings Lance moves into the run** (5.5, 1.11). The Lance
  mirror at `local/findings/` becomes `runs/{id}/lance/findings/`,
  built from the committed `.kenn/findings/<id>.json` records by
  the indexer pass that creates the run — same lifecycle as the
  code-graph Lance datasets (`knowledge`, `aggregate_*`, etc.).
  Reads go through the `live` symlink. Pre-first-index reads
  return "no live snapshot" exactly as code-graph reads do today;
  there is no separate workspace-bound Lance fallback.
  `Layout::findings_local_dir()` is removed.
- **Embed lock moves into the run** (3.9, 1.13). The per-snapshot
  flock at `derived_root/embed-locks/<snapshot-id>` becomes
  `runs/{id}/embed.lock`. The lock semantics are unchanged — it
  still prevents two `embed_pending` calls from concurrently
  filling NULL embeddings on the same run's Lance (a Lance
  correctness concern, not just dedup). Co-locating it with the
  run dir means the lock file is removed naturally when the run
  is gc'd. The `embed-locks/` parent directory is no longer
  created. `Layout::embed_lock_path(snapshot_id: &str)` is removed
  in favor of a per-run path derivable from `run_dir(id)`.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

(none — `store-layout` already describes the destination. The
implementation catches up.)

## Impact

- **Code**:
  - `crates/kenn-indexer/src/driver.rs` (path construction + caller plumbing)
  - `crates/kenn-store/src/db/findings/store.rs` (build path against the run)
  - `crates/kenn-store/src/db/mod.rs` (`embed_pending` lock path)
  - `crates/kenn-store/src/layout.rs` (remove two accessors; the embed-lock
    path is now derived from `run_dir(id)` at the call site)
  - `crates/kenn-indexer/src/pipeline.rs` (build findings Lance during the run)
- **Spec**: no spec edits — the `store-layout` requirements already
  describe these placements.
- **On-disk migration**: not needed. A fresh `kenn index` writes
  the new locations; any stale `derived_root/embed-locks/` or
  `derived_root/findings/` directory from a prior layout is dead
  weight that can be removed at indexer startup (best-effort, no
  user action required).
- **Dependencies**: no new crates. No kenn-dotnet driver changes
  (kenn-dotnet writes JSONL to stdout; the file is on the Rust
  side).
