# Design — watcher-driven, git-free read path

## Evidence (live process sample)

Sampling the stalled `kenn mcp` (pid at 0.0% CPU, parked in `condvar wait` — blocked, not computing) showed the active frames:

```
  kenn_mcp::indexing::poll_once
    → refresh_is_stale_cache
      → kenn_store::staleness::compute_staleness_key
        → git_dirty_files  (git status --porcelain: 658 entries)
        → std::fs::read     (hashing 26.5 MB; 628 were indexers/kenn-ts/node_modules/**)
```

So the stall is blocking git+fs work on the freshness path — not transport, not query, not model inference.

## Current architecture (before)

```
  Watcher (opt-in, watch_start)  : source events → debounce → spawn_background_reindex
  Timer poll (always, every tick): refresh_is_stale_cache → compute_staleness_key (GIT, hashes untracked)
                                    + Store::open + live_target → hot-reload if a newer run published
  Reindex                        : one-writer flock (index.lock) coalesces across instances
  Readers                        : per-process <pid> flock markers pin live snapshots
```

The query/dispatch path is already clean (it serves the `ArcSwap` reader of the current run). The git contact is in **startup** and the **timer poll** — both off the query path but on the runtime, starving it.

## Decisions

### D1. Read path = current open run, nothing else
Query dispatch and `get_index_status` SHALL read only from the in-memory reader of the currently-open run plus process-local cached state. No `compute_staleness_key`, no `Store::open`, no `live_target`, no git subprocess on any read. (Query dispatch already complies; this pins it and removes the remaining contact from `get_index_status`'s freshness source.)

### D2. Watcher is always-on and the primary freshness driver
`kenn mcp` SHALL auto-start the in-process watcher at startup (today it is opt-in via `watch_start`; the timer is the always-on one). The watcher filters by source extension + exclude globs (it does **not** consult git), so it observes any indexed source change including gitignored generated files. The timer poll is removed as the primary mechanism (see D5 for the residual backstop).

### D3. Hot-reload driven by the `live`-pointer event (filter exception, not a second watch)
The watcher already registers `watcher.watch(ws_root, RecursiveMode::Recursive)` (watcher.rs:229), so it **already receives** `.kenn/local/live` events — they are merely dropped today by the `WORKSPACE_SKIP_DIRS` filter (`.kenn/` is excluded). So the change is a **precise filter exception for exactly the `live` pointer**, not a new watch.

`live` is a **symlink** (the store retargets it on publish; `symlink_metadata` confirms, worktree.rs:287). On a `live` event the server hot-reloads the new run, with self-publish dedup (`if resolved(live) == current { no-op }`).

**Feedback-loop guard (load-bearing):** the exception MUST cover **only** the `live` pointer. The recursive watch also sees every file the indexer writes under `.kenn/local/runs/**`; those MUST stay filtered, or a reindex would trigger itself. So: exclude all of `.kenn/` except the single `live` entry.

### D4. `is_stale` is a generation comparison (not a set/clear bool)
`is_stale` SHALL be derived by comparing a monotonic **event sequence** to the **published-run sequence** — `is_stale = last_event_seq > run_event_seq` — not a bool toggled set-on-event / clear-on-publish. A plain bool loses a change that lands *mid-reindex* (event after the indexer read files, before publish): publish would clear the flag though a newer unindexed change exists. `last_event_seq` is a **per-process atomic**, incremented by this instance's watcher; it is meaningless across instances. `get_index_status` reads both counters (atomics) with no git work. Semantics remain *change-seen since the run's start snapshot* (a revert to the indexed state still reads stale until reindex — accepted).

`run_event_seq` is **in-memory only** — a `u64` held alongside the reader binding, never persisted. Nothing reads a per-run on-disk copy: cross-instance reload and restart both ignore any author's stamp (below), and the authoring process sets its own in-memory value at the swap. The **only** per-run persisted freshness artifact is the staleness key (for the seed/backstop, below). There are **two swap paths**, split by provenance because the counter is process-local:

- **Self-publish swap** — the **reindex-completion** path (this instance won the try-lock and reindexed). The reindex SHALL capture this instance's `last_event_seq` at its **start**, hold it in a local var, and apply it to the in-memory `run_event_seq` at its own swap. Any event after capture leaves `last_event_seq > run_event_seq`, so `is_stale` stays true and the next debounce reindexes. This is what closes the mid-reindex lost-update. (This instance's own `live` event is a no-op via the `resolved(live) == current` dedup, so it does not also drive the cross-instance path.)
- **Cross-instance swap** — the **`live`-watch** path, after the self-dedup (this instance reloads a run another process published). There is no commensurable counter to read, so the swap SHALL set `run_event_seq := last_event_seq` snapshot at reload time ("caught up as of now"). A change that raced the cross-instance publish (an event we saw whose content didn't make it into the other process's run) is reconciled by the **backstop** (D5), not the counter — morally a dropped event from this instance's view, which the backstop already covers. **Accepted limitation.**

**Bridging the git key into the counter (seed + backstop).** The startup seed and the D5 backstop both compute a **git staleness key**, but `is_stale` is a *counter* comparison — a key result cannot directly assign `run_event_seq`. The bridge is: a key-compare that reports **stale SHALL synthesize an event**, so the single `is_stale = last_event_seq > run_event_seq` comparison stays the one authoritative signal. "Synthesize an event" names **two required effects**, identical to a real fs event: it both increments `last_event_seq` (flipping `is_stale`) **and** drives a debounced try-lock reindex. Bumping the atomic alone would flip `is_stale` but never reindex, defeating the backstop's purpose; both effects are mandatory. Crucially the seed/backstop achieve these by **invoking the reindex trigger directly** (`spawn_background_reindex`), *not* by feeding the notify watcher's pipeline — the notify watcher and the backstop have different lifetimes (`watch_stop` tears down the former; the backstop survives, see *Tradeoffs*), so the backstop MUST NOT depend on the watcher being alive. No second `is_stale` code path. (This requires each run to record the staleness key it was built against — `kenn index`'s force-skip check already computes it; persist it on the run for the seed/backstop to compare against.)

The initial open on reaching `Ready` initializes `run_event_seq := last_event_seq` (0), same as a cross-instance swap; until the seed lands `is_stale` is optimistically `false`, and the seed reconciles a change made while the server was down.

**Startup seed:** on first reaching `Ready` the watcher has observed no events, so `is_stale` would read `false` even if the workspace changed while the server was down. The server SHALL run **one background `spawn_blocking` key-compare** against the served run on reaching `Ready` (off the read path, per D1/D5); a stale result bumps `last_event_seq` per the bridge above. Until it completes, `is_stale` is optimistically `false` and the backstop reconciles.

```
  source change ─┬─► A.watcher ─► try-lock index.lock ─► WINS ─► reindex ─► flip `live`
                 ├─► B.watcher ─► try-lock ─► held ─► bail (non-blocking)
                 └─► C.watcher ─► try-lock ─► held ─► bail
        `live` flips ──────────┴───────────────────────────► A,B,C `live`-watch fires → each hot-reloads
```

### D5. Low-frequency git backstop on `spawn_blocking`
OS watchers can drop events (heavy churn, some filesystems). A safety re-check SHALL run the staleness key on `spawn_blocking` (never a dispatch worker), compare it to the served run's recorded key, and on **mismatch synthesize an event** (per the D4 bridge: increment `last_event_seq` *and* invoke the reindex trigger directly — not a bare atomic bump, and not via the notify watcher, so it still works after `watch_stop`) so `is_stale` flips true and a reindex fires — closing the window where a dropped watcher event would leave `last_event_seq == run_event_seq` and under-report staleness. Its cadence SHALL be a **config knob** (`mcp.staleness_backstop_secs`, default **300s**) — long enough that idle workspaces do negligible git work, short enough to bound missed-event staleness. Setting it to `0` disables the backstop (watcher-only). **Limitation (accepted):** the git form cannot see gitignored generated files — those rely on the watcher (D2). The backstop is a floor for *missed watcher events on tracked files*, not a replacement for the watcher. It also reconciles the startup window (D4 seed) and any change that raced a cross-instance publish (D4 cross-instance swap).

### D6. Multi-instance: coalesce, never redundant-block
Each instance watches source + `live` independently. On a change each attempts reindex via a **non-blocking try-lock** on the one-writer `index.lock`; the winner reindexes and flips `live`, losers bail immediately (no blocking wait) and reload via the `live` watch (D3). Only the winner computes a staleness key in this **change-triggered reindex path** (the trigger is a counter bump; only the winner reaches the reindex skip-check). The D5 backstop and D4 startup seed are separate per-instance paths that each compute a key on their own cadence. This reuses the existing flock + `<pid>` reader markers unchanged.

### D7. `compute_staleness_key` excludes untracked files
The git form SHALL hash **tracked-modified files only** (drop the untracked set from `git status`). Untracked scratch (node_modules, build output, tmp clones) can then never inflate the key or the hash cost — the durable bound under D5 and any reindex skip-check. The non-git stat-tree digest form (for non-git workspaces) is unchanged.

## Tradeoffs / risks
- **Watcher reliability** is now load-bearing. D5's backstop covers missed *tracked* events; gitignored generated files depend solely on the watcher firing. If a generated file changes without an OS event, it stays unindexed until the next event — accepted.
- **`live`-watch feedback**: the winner's own publish flips `live` and fires its own watch → deduped by `if resolved(live) == current`. The new-run install MUST update `current` atomically with the reader swap so the dedup holds; and only the `live` entry is un-filtered (D3) so `runs/**` writes never feed back.
- **`watch_stop` degrades freshness** more than before: with the timer gone, stopping the watcher leaves only the D5 backstop (cadence `staleness_backstop_secs`). Intended — `watch_stop` is now "freshness on the slow path," not "freshness off."
- **Failed winner reindex isn't retried by losers**: losers try-lock-and-bail (D6), so if the winner crashes before flipping `live`, no instance reindexes until the next event or backstop tick. Accepted (eventually consistent).
- **`mcp.watch_on` default flips to `true`** (or is obsoleted): the watcher is the primary mechanism, so it starts by default on reaching `Ready`. An explicit `watch_stop` still wins for the session.
- **Debounce windows differ per instance** → brief redundant try-lock attempts; all are cheap no-ops once `live` already satisfies the key.

## Build order
```
  1. compute_staleness_key: tracked-only (D7) — smallest, removes the blow-up at the source
  2. is_stale → event-seq vs run-seq generation comparison (D4); get_index_status reads two atomics, no git; background startup seed
  3. watcher auto-start (D2) + filter exception for the `live` pointer only (keep `runs/**` filtered) driving hot-reload (D3)
  4. remove timer poll; add spawn_blocking backstop with `staleness_backstop_secs` cadence (D5)
  5. losers try-lock-and-bail (D6); verify N-instance converge via `live` watch
```
