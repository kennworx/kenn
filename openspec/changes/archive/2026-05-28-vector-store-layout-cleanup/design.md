## Context

`vector-store-layout` shipped the runs-centric layout. Three small
cleanups remained: move the JSONL stream files into the run, move
the findings Lance mirror into the run, and move the embed lock
into the run. Each was deferred from the parent change because
they reach across module boundaries (indexer driver,
findings-store, embed pass) that the parent change didn't want
to widen.

This design lays out the corrected scope: kenn-dotnet writes
JSONL to stdout (not files), so the "driver refactor" framing
in earlier notes was wrong — the file is Rust-side and only
needs a path-construction change. The findings Lance lifecycle
already matches code-graph Lance — JSON records are the
committed source of truth, the Lance mirror is rebuilt by the
indexer pass, reads go through `live`. The embed lock prevents
real Lance corruption (two concurrent fills on the same run's
NULL-embedding rows), so it stays — only its filesystem
location changes.

## Goals / Non-Goals

**Goals:**

- Move `kenn-dotnet-stream-{pid}-{n}.jsonl` files from
  `derived_root/` into `runs/{id}/`.
- Build the findings Lance mirror at
  `runs/{id}/lance/findings/` during the indexer pass that
  creates the run; route reads through the `live` symlink.
- Remove `Layout::findings_local_dir()`.
- Move the embed pass's flock from
  `derived_root/embed-locks/<snapshot-id>` to
  `runs/{id}/embed.lock`.
- Remove `Layout::embed_lock_path(snapshot_id: &str)` in favor
  of deriving the path from `run_dir(id)` at the call site.
- Keep `just crap-ci`, the full workspace test suite, and
  `just embed-smoke` green.

**Non-Goals:**

- kenn-dotnet driver changes. The .NET side writes to stdout;
  the Rust side captures and writes the file. Only the file
  location changes.
- Removing the embed lock. Lance fill on a NULL column under
  two concurrent writers is an unknown-correctness path; the
  lock is the existing protection and stays.
- A bootstrap-run for pre-first-index findings. Findings reads
  through Lance follow the same "no live → no result" rule as
  code-graph reads; `kenn find` writes JSON records that are
  picked up by the next indexer pass.
- Designing `kenn gc`. That's the future maintenance command
  for unreferenced vector sidecar files; this change has nothing
  to do with it.

## Decisions

### A. JSONL stream files: keep the pid+counter, only move the directory

`make_scratch_stream_path` in `crates/kenn-indexer/src/driver.rs`
returns a path under `derived_root/` today. The naming
(`kenn-dotnet-stream-{pid}-{n}.jsonl`) is fine — pid disambiguates
concurrent indexers in shared `derived_root` configurations, the
counter disambiguates retries within one process.

Change: the function takes the active run dir instead of
`derived_root`, and returns
`runs/{id}/kenn-dotnet-stream-{pid}-{n}.jsonl`. Caller plumbing
threads the run dir down from the indexer pass — the indexer's
pass-init already has the run dir in hand.

Alternative considered: one file per language
(`runs/{id}/{lang}.jsonl`). Rejected — multiple retries within a
pass produce multiple files; collapsing them would mean either
overwriting or appending, neither of which is what the current
retry logic wants. The counter pattern works; just relocate it.

### B. Findings Lance: ACTIVE-RUN model with publish-fenced lock

The first cut of this design said "same lifecycle as code-graph
Lance — built by indexer pass, read via live, never touched by
`store_finding`." That broke the existing contract that
`store_finding` is immediately searchable in the same MCP
session: today the MCP tools at `kenn-mcp/src/tools.rs:1601,
1667` call `FindingsStore::store_finding` followed immediately
by `flush()`, which today rebuilds the workspace-bound Lance.

After conversation, the correct shape is the **active-run model
with cross-process synchronization** and a **sync-BM25 /
async-vector write path** that mirrors the code-graph
index/embed split.

Nothing about the *location* is reversed: the findings Lance lives
at `runs/{id}/lance/findings/`, read and written through the `live`
symlink — exactly like `lance/knowledge/`. `live` is the stable
handle; it points at whatever the current run is. Embeddings
survive a code reindex via the content-addressed findings vector
sidecar (`Layout::findings_vectors_dir()`, fingerprint-keyed): a
fresh run rebuilds the findings Lance from the committed JSON
records but reconciles vectors from the sidecar — fingerprint hits
reuse, no re-embedding.

What changes from the first cut is the *write mechanism*. The old
plan rebuilt the entire Lance on every `flush()` (re-read all
records, re-embed, rebuild every index, atomic-swap). That is O(N)
per finding. The new write path is:

```
WRITE PATH (store_finding, sync, via live):
  acquire <derived_root>/findings-publish.lock  (POSIX flock,
                                                 same crate as
                                                 embed.lock)
  write .kenn/findings/<id>.json                (committed source)
  if live target exists:
      append the new finding row to live/lance/findings/
        (WriteMode::Append — same primitive the graph store uses)
      BM25 / FTS index: updated SYNC so the row is keyword-
        findable on return                      (see Open item L)
      embedding column: NULL                     (deferred)
  release lock

EMBED PATH (embed pass, async, via live):
  fill NULL finding vectors in live/lance/findings/
    — reconcile from the findings vector sidecar; embed misses
  optimize_indices (fold appended fragments into the indexes)
  guarded by runs/{id}/embed.lock                (Block C)

BUILD PATH (indexer pass at end of run):
  build runs/{id}/lance/findings/ from records dir
    (reusing sidecar vectors)
  acquire findings-publish.lock
  catch-up scan: re-read records dir, append any records that
                 appeared since the build started
  atomic flip live → runs/{id}
  release lock
```

A finding is **searchable without any `kenn index` pass**: the
WRITE PATH puts it into the live Lance directly. The BUILD PATH is
only exercised when a *code* reindex creates a new run — it
repopulates the findings Lance into that run (cheap, vectors from
the sidecar) so the live-flip doesn't drop findings from the
mirror. Keyword search works immediately (sync BM25); semantic /
vector search over a given finding lights up only after the async
embed pass fills its vector — identical to how a freshly-indexed
code symbol is keyword-findable before `kenn embed` runs.

The same `findings-publish.lock` fences the WRITE and BUILD
critical sections. A `store_finding` mid-publish blocks until the
flip completes, then appends to the new live. A publish mid-append
blocks until the append completes, then catch-up picks up the new
record. Zero race window from the user's seat: a write-then-search
round trip in one MCP session always sees the write.

The lock lives in `derived_root` (gitignored runtime state),
sibling to `index.lock` and `live`. Cross-process and CLI-safe by
virtue of flock semantics (kernel-managed inode lock).

Per-worktree semantics:
- Default layout: each worktree has its own `derived_root` →
  its own lock → independent.
- `[layout] derived_root = "global"`: shared derived_root → shared
  lock → correctly serialized (worktrees contend on the same `live`).

Alternative considered: process-local `Mutex<()>`. Rejected — the
multi-instance case (two `kenn-mcp` instances; `kenn-mcp` + `kenn
index` from a terminal) is real and proven by the existing
`two_instances_both_reach_ready_on_same_workspace` test.

Alternative considered: strict "kenn index builds Lance, store_finding
writes JSON only" — rejected because it breaks the immediate-search
contract. The active-run model preserves the contract.

Alternative considered: full rebuild on every `flush()` (the first
cut). Rejected — O(N) re-embed + index rebuild per finding, when an
append + sync FTS update is O(1) and the embed pass already exists
to fill vectors asynchronously.

**Open item L — RESOLVED (yes).** Spike
`kenn-store/src/db/lance/index.rs::fts_finds_appended_unindexed_row`
(Lance 6.0.0): a row appended via `WriteMode::Append` AFTER the FTS
index was built is returned by `full_text_search` without any
`optimize_indices` — Lance does a flat fallback over the unindexed
fragments. Therefore "sync BM25" in the WRITE PATH is just the
`WriteMode::Append`; the row is keyword-findable on return.
`optimize_indices` is a pure performance fold, deferred to the async
embed pass alongside vector fill. A regression test guards this
behavior so a future Lance upgrade that changes it fails loudly.

### B (legacy framing — now superseded by §B above):

The committed source of truth for findings is the per-finding
JSON record at `.kenn/findings/<id>.json`. Today
`FindingsStore::open` builds a Lance mirror at
`derived_root/findings/` (the path
`Layout::findings_local_dir()` returns). That mirror is
workspace-bound — opened pre-first-index for `kenn find` reads.

New shape:

```
WRITE:  kenn find  →  .kenn/findings/<id>.json
                      (committed; this stays unchanged)

BUILD:  kenn index pass {id}
          → reads the JSON records
          → builds runs/{id}/lance/findings/
          → completes the run; live symlink flips

READ:   FindingsStore::open
          → resolves the live target (snapshot dir)
          → opens snapshot_dir/lance/findings/
          → returns "no live snapshot" if live is absent
            (same error as code-graph reads)
```

Pre-first-index `kenn find` writes the JSON but cannot serve
reads until the first indexer pass runs and publishes a live
target. This matches code-graph behavior; no separate
workspace-bound fallback is needed.

Alternative considered: keep `findings_local_dir()` as a
workspace-bound fallback that gets read when no live target
exists. Rejected — it forks the read path into two cases
(via-live vs via-fallback), produces inconsistent staleness
semantics (the fallback would lag the live target on rebuilds),
and breaks the runs-centric invariant. Cost of "no result
pre-first-index" is bounded: `kenn index` is the first thing
anyone does on a new workspace.

### C. Embed lock: relocate, don't remove

`embed_pending` at `crates/kenn-store/src/db/mod.rs:140` is a
background pass that fills NULL embeddings on a run's
`lance/knowledge/`. The flock at
`derived_root/embed-locks/<snapshot-id>` serializes two
concurrent embed passes against the same run.

The lock prevents two distinct problems:

1. **Wasted compute.** Both processes embed the same fingerprints.
   Content-addressing makes the resulting sidecar writes safe
   (byte-identical files, rename collisions are no-ops), so the
   cost is only GPU/CPU cycles. Real but not severe.
2. **Lance NULL-fill correctness.** Both processes call
   `embed_pending_batches` against the same NULL rows. The Lance
   semantics under "two writers updating the same NULL column"
   is not a guaranteed-safe operation — duplicate rows, partial
   fills, or undefined final state are all on the table. This
   is the correctness concern that justifies keeping the lock.

Conclusion: keep the lock. Move it to `runs/{id}/embed.lock` so
it lives with the data it protects. Co-location means:

- The lock file is destroyed naturally when its run is gc'd by
  `lifecycle::gc()` — no separate cleanup pass for orphan locks.
- The `embed-locks/` parent directory is no longer needed and
  doesn't get created.
- The accessor signature simplifies: instead of
  `Layout::embed_lock_path(snapshot_id: &str) → derived_root/embed-locks/<id>`,
  the call site derives `run_dir.join("embed.lock")` directly
  from the run dir it already has.

Alternative considered: replace the flock with a process-local
`Mutex<HashSet<RunId>>`. Rejected — the cross-process case isn't
academic. Two kenn-mcp instances on the same workspace (rare but
possible: an editor restart racing the previous instance's
shutdown) would both run `embed_pending` on the same snapshot.
A process-local mutex doesn't cover that.

Alternative considered: drop the lock entirely. Rejected per the
Lance correctness concern in #2 above. Measuring the failure
mode is possible (drop the lock, run two concurrent embed passes,
inspect the resulting Lance) but the lock costs almost nothing
to keep and its semantics are clear.

## Risks / Trade-offs

- **Old `derived_root/embed-locks/` and `derived_root/findings/`
  directories linger on upgrade.** → Indexer startup
  best-effort-removes them. Failure is logged, not fatal. A
  user with a multi-GB stale `findings/` directory loses no data
  (the source-of-truth JSONs are in committed root) but reclaims
  the disk only after that cleanup runs.
- **Findings reads return "no live snapshot" pre-first-index.**
  → Matches code-graph reads. Documented in the spec. The user
  workflow is `kenn index` first; this is not a behavior
  regression for anyone.
- **Embed lock relocation changes the path that contended-lock
  diagnostics surface.** → Update the trace/log message to
  reference the new path. Existing tests that don't depend on
  the lock path are unaffected.

## Migration Plan

No on-disk migration is required. A fresh `kenn index` after
this change creates `runs/{id}/embed.lock`,
`runs/{id}/kenn-dotnet-stream-*.jsonl`, and
`runs/{id}/lance/findings/` in their new homes. The next
indexer startup sweeps stale top-level `embed-locks/` and
`findings/` directories from prior layouts.

## Open Questions

- ~~**Open item L (§B):** does Lance 6's `full_text_search` return
  rows in freshly-appended fragments via a flat fallback?~~
  **RESOLVED (yes)** — see §B. "Sync BM25" is just `WriteMode::Append`;
  `optimize_indices` is deferred to the async embed pass.
