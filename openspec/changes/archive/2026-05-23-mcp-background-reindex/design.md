## Context

The `kenn mcp` server (`crates/kenn-mcp`) decides Skip-vs-Reindex once in
`run_startup_decision`, binds a `DbReader` to a **concrete** snapshot directory
via `open_ready`, and transitions to a terminal `Ready`. `IndexStatus.is_stale`
and `reindex_in_progress` are hard-coded `false`. The store
(`crates/kenn-store`) already provides the primitives this change needs: a
one-writer `index.lock` (`fs2`-based, auto-released on process death), a
`live/` pointer to the current snapshot, staleness keys
(`compute_staleness_key` / `StalenessKey::matches`), and snapshot GC bounded by
`[lifecycle] gc_keep`.

Each Claude session spawns its own `kenn mcp` process, so N servers routinely
share one `.kenn/` store. The design must be correct under concurrency, not
just single-process.

## Goals / Non-Goals

**Goals:**
- A running server picks up a newer snapshot (`live/` repointed) without restart.
- A `reindex` MCP tool triggers an in-process reindex that never blocks reads.
- `get_index_status` reports real `is_stale` / `reindex_in_progress` + progress.
- Correct under multiple concurrent `kenn mcp` instances on one workspace.
- A snapshot in use is never GC'd out from under any instance.

**Non-Goals:**
- The filesystem watcher that auto-triggers reindex on edits — that is the
  separate `file-watcher-reindex` change. This change only ensures the MCP
  server *reacts* to new snapshots; it does not decide when to produce them
  beyond the explicit `reindex` tool.
- Changing the indexing pipeline (`kenn-indexer`) itself.
- Networked / multi-machine stores.

## Decisions

### 1. Swap the reader, don't rebuild the lifecycle

`LifecycleState::Ready` keeps the reader behind a swappable cell —
`arc_swap::ArcSwap<DbReader>`. Tool dispatch calls `read.load_full()`
(lock-free) to obtain an `Arc<DbReader>` it owns for the call's duration;
a snapshot swap calls `read.store(new)`, also lock-free, and never blocks
readers. In-flight calls finish against the snapshot they started on;
later calls see the new one.

*Alternative — `std::sync::RwLock<Arc<DbReader>>`:* the same shape using
only the standard library, kept on first cut to avoid a new dependency.
Dropped at apply time once it was clear `arc-swap` is the canonical
solution for this pattern (widely used in `tracing-subscriber`, `log`,
etc.) and the std form was just boilerplate around the same idea —
`Arc::clone(&read.read().expect("poisoned"))` at every dispatch site.
The dependency is one small crate (≈1 kloc, no transitives) and
delivers truly lock-free reads.

*Alternative — replace the whole `LifecycleState::Ready`:* churns the
lifecycle `RwLock` and races in-flight calls. Rejected.

### 2. Detect new snapshots by polling the `live/` pointer

A background task on the rmcp runtime polls the resolved `live/` target every
~3 s. If it names a snapshot directory different from the one the reader holds,
the task opens a reader against it and performs the swap (Decision 1).

*Why polling:* the `live/` pointer is the **shared cross-process signal** — one
instance's reindex, a `kenn index` CLI run, and this server's own `reindex` all
publish the same way, so a single poll covers every source for free. A `notify`
watcher would add a dependency, couple this change to `file-watcher-reindex`,
and watching a symlink/dir swap is finicky. Indexing takes minutes; a few
seconds of detection lag is irrelevant.

The same poll tick also recomputes the workspace staleness key
(`compute_staleness_key`) and caches the `is_stale` result, so `get_index_status`
reads a cached value and never does git work on the call path.

### 3. Reindex coordination = the existing one-writer lock

The `reindex` tool (and cold-start reindex) attempt `begin_indexing`, which
acquires `index.lock`. Acquired ⇒ run the pipeline on a `spawn_blocking`
thread, publish, let the poll swap the reader. Not acquired ⇒ another instance
or a `kenn index` CLI run is already reindexing; **do not error** — report the
in-progress run and let the poll hot-reload the winner's snapshot. This makes
reindex coalescing cross-process with no new machinery.

### 4. Cross-process GC pin = a store-level `flock` reader registry

Reader pins live in a store-level registry, **not** inside snapshot
directories — `.kenn/local/readers/<snapshot-id>/<pid>` — so published snapshot
directories stay immutable. When a server opens snapshot `S` it creates and
`flock`s its `<pid>` marker under `S`'s registry entry (`fs2`, already a
dependency). Snapshot GC, before evicting any snapshot beyond `gc_keep`, probes
that snapshot's registry markers with a non-blocking exclusive `flock`: success
⇒ the holder is dead, reclaim the marker; failure ⇒ a live reader holds it,
skip the snapshot. `flock` auto-releases on process exit, so a crashed instance
never leaks a permanent pin. The marker is created *before* the reader is
considered live; if the target snapshot vanished in the race, the server
re-resolves `live/`.

### 5. `Ready` stays non-terminal, with nested reindex status

`LifecycleState::Ready` gains `reindex: Option<ReindexProgress>`. A background
reindex does **not** introduce a `Reindexing` state — that would make
`Ready`-gated tools return `INDEX_UNAVAILABLE` mid-reindex, which violates the
"reads never blocked" goal. Reindex is a sub-state of `Ready`. A
background-reindex error clears `reindex` and leaves the prior reader in place;
it never reaches `Failed` (only cold start can). The `reindex` tool also
doubles as the recovery path *out* of `Failed` — `Failed → Indexing` — so a
transient cold-start failure no longer forces a process restart.

### 6. Coordinate the embed job — at every trigger, not just hot-reload

`kenn index` writes structural data but leaves embedding vectors null — the
post-index embed job (`spawn_embed_job` → `embed_pending`) fills them. The
server triggers it after a cold-start `Ready` (the existing call) and after
every hot-reload swap (new).

Both triggers MUST be coordinated cross-process. If N instances cold-start or
hot-reload onto the same null-embedding snapshot and each spawns `embed_pending`,
they each run the expensive llama.cpp inference over the whole snapshot and
contend on concurrent writes to its vector column — "idempotent" does not make
that acceptable. The coordination therefore lives **inside the embed job
itself**, not in the hot-reload caller, so every call site — cold-start,
hot-reload, and the `kenn embed` CLI — is covered uniformly: the job acquires a
lock, the acquirer embeds, the rest skip. Because `embed_pending` republishes
the store in place, the skipping instances — already reader-bound to that
snapshot — observe the vectors appear with no reopen (the mechanism
`indexing.rs` already documents). One embed run per snapshot; every instance
benefits.

Use a **separate per-snapshot embed lock**, not the store one-writer
`index.lock`. Embedding takes minutes; if it held `index.lock` it would block
every `reindex` for that whole duration. A distinct embed lock serializes embed
against embed while letting reindex and embed proceed independently.

Because `index.lock` is no longer taken during embed, the embed job
also acquires a reader-registry pin (Decision 4) on its target snapshot
for the duration of the run. Without that pin a concurrent reindex could
publish a newer snapshot, the server hot-reload off the embed's target,
and a third instance's GC evict it mid-write — POSIX would keep the
inode alive for the embed's open fds but the published vectors would
land in nameless storage and be lost. The pin closes that gap with one
extra `flock` per embed run.

`embed_pending`'s no-op-when-already-embedded behavior is assumed from its name
and the `incremental-embedding` design; the apply phase MUST confirm it and add
an explicit "already embedded ⇒ skip" guard if it does not already hold.

## Risks / Trade-offs

- **Hot-reload lag up to one poll interval** → small interval (~3 s); negligible
  next to multi-minute indexing.
- **`flock` portability** → kenn already depends on `fs2` `flock` for
  `index.lock`; this stays within the same portability envelope (macOS/Linux
  first-class).
- **Cursor spanning a swap goes stale** → the existing `STALE_CURSOR` error
  already covers index rotation; hot-reload reuses it, no new contract.
- **GC-vs-open race** (GC probes `S` between another instance resolving `live →
  S` and creating its marker) → create+`flock` the marker before the reader is
  published; on a lost race, re-resolve `live/` and retry.
- **Two instances cold-start at once** → both race for `index.lock`; the loser
  waits and hot-reloads — already covered by Decision 3.
- **New snapshot fails to open** (corrupt or partially published) → the swap is
  all-or-nothing: keep the current reader in service, log, and retry on the
  next poll. A failed open never blanks the server.
- **Cached `is_stale` lags one poll tick after a swap** → right after a reader
  swap the cached `is_stale` still reflects the previous snapshot until the next
  poll (~3 s) recomputes it against the new one. Self-correcting; an accepted
  small window.

## Migration Plan

Purely additive: new background task, new tool, swappable reader in `kenn-mcp`;
a GC-pin hook + the reader registry in `kenn-store`. No data migration —
existing snapshots and `index.lock` semantics are unchanged. Rollback is a
straight revert; older `kenn mcp` binaries simply ignore the
`.kenn/local/readers/` registry (GC there keeps its old `gc_keep`-only
behavior).

## Open Questions

- Poll interval — fix at 3 s, or expose under `[mcp]` in `kenn.toml`?
- Should `reindex` take a `force` argument mirroring `kenn index --force`
  (bypass the git-aware staleness skip)?
