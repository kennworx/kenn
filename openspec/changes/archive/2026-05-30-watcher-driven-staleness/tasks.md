# Tasks — watcher-driven-staleness

## 1. Bound staleness at the source
- [x] 1.1 `compute_staleness_key`: hash tracked-modified files only; drop untracked from the git form (D7) → verify: an untracked `node_modules`/scratch file does not change the key and is never read
- [x] 1.2 Unit test: dirty tracked file changes the key; new untracked file does not → verify: green

## 2. Git-free read path
- [x] 2.1 `is_stale` via generation compare: per-process `last_event_seq` (bumped by the watcher) vs an **in-memory** `run_event_seq` (held with the reader binding, never persisted per run); `is_stale = last_event_seq > run_event_seq`. Update split by provenance: **self-publish** (reindex-completion path) applies `last_event_seq` captured at reindex start; **cross-instance** swap (`live`-watch path, after self-dedup) sets `run_event_seq := last_event_seq` at reload (D4) → verify: (a) event mid-reindex still leaves `is_stale` true after self-publish; (b) after a cross-instance reload, a new local event flips `is_stale` true (not masked by the other process's value)
- [x] 2.2 Staleness key already persisted per run (`kenn index` meta.json); key→counter bridge: a stale key-compare (startup seed or backstop) **synthesizes an event** — increments `last_event_seq` AND invokes the reindex trigger directly (NOT via the notify watcher, so it survives `watch_stop`); not a bare atomic bump. Initial open sets `run_event_seq := last_event_seq`. Startup seed = one `spawn_blocking` key-compare on reaching `Ready` (D4) → verify: a change made while the server was down both flips `is_stale` and triggers a reindex after the seed, not silently fresh and not stale-forever-without-reindex
- [x] 2.3 Audit query dispatch + `get_index_status`: assert no `compute_staleness_key` / `Store::open` / `live_target` on the read path (D1) → `get_index_status` now reads `state.is_stale()` (two atomic loads); no git/store-open on the call path
- [x] 2.4 Latency test verified live (this session): `get_index_status` returned instantly after MCP reload; `search_symbols` returned 10 hits in well under a second on the working tree post-reindex.

## 3. Watcher-driven freshness
- [x] 3.1 Auto-start the in-process watcher on `kenn mcp` startup (D2) → `watch_on` defaults true; autostart waits for `Ready` then starts
- [x] 3.2 Filter exception for **exactly** the `live` symlink (the recursive watch already sees it; keep `.kenn/local/runs/**` filtered); on a `live` event hot-reload with self-publish dedup `if resolved(live) == current` (D3) → verify: `external_publish_is_hot_reloaded` + `instances_converge_on_newest_snapshot` pass via the live path
- [x] 3.3 Remove the timer snapshot-poll as the primary mechanism (D2/D3) → `start_snapshot_poll_task`/timer removed; hot-reload is `live`-event-driven, staleness is backstop-driven

## 4. Backstop + multi-instance
- [x] 4.1 Backstop: key-compare on `spawn_blocking` at `mcp.staleness_backstop_secs` cadence (default 300; 0 disables); on mismatch synthesize an event (per D4 bridge — increments `last_event_seq` AND invokes the reindex trigger directly, independent of the notify watcher) so `is_stale` flips and a reindex fires (D5) → verify: a missed-event tracked change is eventually reindexed and `is_stale` flips before it; the backstop still reindexes after `watch_stop`; runs off the dispatch runtime; idle workspace does no git between ticks
- [x] 4.2 Reindex trigger uses a non-blocking try-lock on the one-writer flock; losers bail and reload via the `live` watch (D6) → `index_workspace`/`begin_indexing` flock + `LockHeld` coalesce already; `instances_converge_on_newest_snapshot` passes
- [x] 4.3 Document the accepted limitation: git backstop misses gitignored generated files (watcher covers them) → noted in `start_staleness_backstop_task` doc + spec/design

## 5. Gates
- [x] 5.1 `cargo clippy --workspace --all-targets` clean; `just crap-ci` PASSED; `cargo fmt --all`
- [x] 5.2 Stall reproduction gone — empirically verified this session: post-MCP-reload `get_index_status` is instant, `search_symbols` returns sub-second; the original 10-min hang (which triggered the watcher-driven-staleness change) does not reproduce. Read path is atomic, timer poll is removed, key runs only on the 300s backstop `spawn_blocking`.
