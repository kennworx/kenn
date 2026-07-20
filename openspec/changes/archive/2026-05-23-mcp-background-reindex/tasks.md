## 1. Store: cross-process reader markers + GC pin

- [x] 1.1 Add a reader-registry helper in `kenn-store`: on opening snapshot `S`, create and `flock` (`fs2`) a `<pid>` marker in a store-level registry at `.kenn/local/readers/<snapshot-id>/` (keeps snapshot dirs immutable); expose a guard type that releases on drop.
- [x] 1.2 Make snapshot GC pin-aware: before evicting a snapshot beyond `[lifecycle] gc_keep`, probe that snapshot's registry markers with a non-blocking exclusive `flock` — reclaim dead markers, skip the snapshot if any live marker remains.
- [x] 1.3 Unit-test GC: a snapshot with a live marker survives a sweep; the same snapshot is collected once the marker is released / the holder is gone.

## 2. MCP: swappable reader

- [x] 2.1 Change `LifecycleState::Ready` to hold the reader as `arc_swap::ArcSwap<DbReader>` (swappable) plus `snapshot_path` / `snapshot_id` / `indexed_at` for the currently-served snapshot. (Switched from `RwLock<Arc<DbReader>>` to `arc-swap` during apply — see design Decision 1.)
- [x] 2.2 Update tool dispatch to fetch the reader via `read.load_full()` — lock-free; the call owns its `Arc<DbReader>` clone with no lock held across awaits.
- [x] 2.3 Add `reindex: Option<ReindexProgress>` to `Ready`; `Ready` is non-terminal (a background reindex never transitions to `Failed`).

## 3. MCP: snapshot hot-reload

- [x] 3.1 Add a background poll task (interval ~3 s) that resolves `.kenn/live/` and compares it to the served snapshot.
- [x] 3.2 On a newer snapshot: open a `DbReader` (acquiring its reader marker per 1.1), then atomically swap it into `Ready` so in-flight calls finish on the old snapshot and later calls use the new one.
- [x] 3.3 Release the old snapshot's reader marker after the swap.
- [x] 3.4 On a failed reader-open of a newer snapshot, keep the current reader in service, log, and retry on the next poll — the swap is all-or-nothing and never blanks the server.
- [x] 3.5 Move embed coordination into the embed job itself: it acquires a separate per-snapshot embed lock (not `index.lock`), embeds if acquired, skips if not — so cold-start, hot-reload, and `kenn embed` are all coordinated. Trigger it after every successful swap.
- [x] 3.6 Route the cold-start embed call (`run_startup_decision` → `spawn_embed_job`) through the same coordinated job; confirm `embed_pending` is a clean no-op when the snapshot is already fully embedded, and add an explicit "already embedded ⇒ skip" guard if it is not.
- [x] 3.7 Integration test: a separate `kenn index` run is hot-reloaded — `get_index_status` reports the new `snapshot_id`/`indexed_at`; an in-flight call started before the swap completes against the old snapshot.
- [x] 3.8 Integration test: two instances cold-starting (and two hot-reloading) onto the same null-embedding snapshot run exactly one embed pass between them; a corrupt/partial newer snapshot does not blank the server.

## 4. MCP: background reindex + `reindex` tool

- [x] 4.1 Add a `reindex` tool (`tools.rs` + `server.rs` registration) that triggers an in-process reindex and returns promptly without blocking reads.
- [x] 4.2 Coordinate via the store one-writer lock: acquire `index.lock` → run the pipeline on `spawn_blocking`, publish, let the poll (section 3) swap the reader; cannot acquire → return an "already in progress" result, no error.
- [x] 4.3 Coalesce a second `reindex` call while one is running — including a call received during cold-start `Indexing` — into a no-op that reports the in-progress run.
- [x] 4.4 On background-reindex error: clear `Ready.reindex`, keep the prior reader, surface the reason via `get_index_status`; do not enter `Failed`.
- [x] 4.5 Integration test: `reindex` runs without blocking concurrent tool calls; a second instance's `reindex` during the first's run does not start a competing pipeline and hot-reloads the result.
- [x] 4.6 Allow `reindex` from `Failed`: transition `Failed → Indexing` and retry the pipeline as a recovery path; non-status tools return `INDEX_UNAVAILABLE` until it reaches `Ready`. Integration test: a `Failed` server is recovered to `Ready` by a `reindex` call.

## 5. MCP: real index status

- [x] 5.1 Add reindex-progress fields to `IndexStatus` (`types.rs`): phase + running counters while a reindex is in flight.
- [x] 5.2 Compute the staleness key on the poll tick and cache `is_stale`; `get_index_status` reads the cached `is_stale` (no git work on the call path) and derives `reindex_in_progress` from `Ready.reindex`; drop the hard-coded `false`s.
- [x] 5.3 Test: stale workspace → `is_stale = true`; mid-reindex → `reindex_in_progress = true` with progress; idle fresh server → both `false`.

## 6. Multi-instance correctness

- [x] 6.1 Integration test: a second `kenn mcp` starts cleanly while a first is `Ready` on the same workspace — both reach `Ready`, neither blocks the other.
- [x] 6.2 Integration test: a snapshot held by instance A survives a GC sweep run by instance B; it is collected only after A releases it.
- [x] 6.3 Integration test: several instances converge on the newest snapshot after any one of them (or a CLI `kenn index`) publishes it.

## 7. Wire-up and docs

- [x] 7.1 Update the `mcp-orchestrated-indexing` spec text and the `kenn-mcp` module docs (`state.rs`, `indexing.rs`) to describe the non-terminal `Ready`, hot-reload, and the `reindex` tool.
- [x] 7.2 Update the `reindex` tool description (≤200 tokens per the design budget) and refresh the tool count in `server.rs` docs.
- [x] 7.3 Run `cargo clippy --workspace --all-targets` and the kenn-mcp integration tests; confirm zero warnings.
