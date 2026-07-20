## Why

MCP read calls can stall for **tens of seconds**. Root-caused by sampling the live `kenn mcp` process: it was parked in `kenn_store::staleness::compute_staleness_key` → `git_dirty_files` + `std::fs::read`, hashing **every file `git status` reports dirty/untracked**. With an untracked `node_modules/` present that was 658 files / 26.5 MB hashed — on the runtime, gating readiness and churned every timer tick.

Two structural problems behind the symptom:

1. **Freshness work is on the hot path.** Startup (`decide_startup_state`) computes the git staleness key *before* going `Ready`, so every query waits on it. The always-on **timer poll** recomputes it every tick (git subprocess + file hashing) and also opens the store to check `live_target` for hot-reload. The read/query path must never do this.
2. **Timer-driven, not event-driven.** A periodic poll does git work whether or not anything changed, and detects another instance's new run only on the next tick. Meanwhile an **in-process file watcher already exists** and already triggers the *same* reindex path — but it's opt-in, and the timer remains the always-on mechanism.

The fix is to make freshness **event-driven off the existing watcher** and keep the **read path serving only the current open run** — never git, never a store open, never a `live_target` probe. The watcher becomes the background job that tracks staleness, triggers reindex, and observes cross-instance publishes.

## What Changes

- **Read path is git-free and current-run-only.** Query dispatch and `get_index_status` serve exclusively from the in-memory reader of the current run plus cached state. No `compute_staleness_key`, `Store::open`, or `live_target` on any read.
- **`is_stale` becomes a generation comparison** — a per-process `last_event_seq` (bumped by the watcher) vs the served run's `run_event_seq`. This avoids the lost-update a set/clear bool would suffer for a change landing mid-reindex. The `run_event_seq` is **in-memory only** (never persisted per run) and its update is **split by reader-swap provenance**: a self-publish (reindex-completion path) applies the counter captured at reindex start; a cross-instance reload (`live`-watch path) snapshots the local counter instead, since another process's counter is incommensurable, leaning on the backstop to reconcile a change that raced the publish. The only per-run persisted freshness artifact is the staleness key. The startup seed and the git backstop compute a staleness key, not a counter — a stale key-compare **synthesizes an event** (bumps `last_event_seq`) so the single comparison stays authoritative. Semantics: *change-seen since the run's start* (not content-differs).
- **Watcher is always-on for `kenn mcp`** (auto-started), replacing the timer as the primary trigger for both reindex and hot-reload.
- **Hot-reload is driven by the `live`-pointer event** — a filter exception for exactly the `live` symlink (the recursive watch already sees it; `.kenn/local/runs/**` stays filtered to avoid self-triggering), with self-publish dedup — not a timer probing `live_target`. This also gives **cross-instance reload for free**: whoever publishes a new run, every instance's watcher sees `live` change and reloads.
- **Low-frequency git backstop** for events the OS watcher may drop (heavy churn / some filesystems), run on `spawn_blocking` off the dispatch runtime. Documented limitation: the git backstop cannot see **gitignored generated files** — those are covered by the watcher (which filters by source extension, not git status).
- **`compute_staleness_key` excludes untracked files** (tracked-modified only), so untracked scratch (node_modules, build output, tmp clones) can never blow up the key — the durable floor under both the backstop and any reindex skip-check.
- **Multi-instance staleness is coalesced, never redundant-blocking.** Each instance watches source + `live`. A change makes each attempt reindex via a **non-blocking try-lock** on the one-writer flock; the winner reindexes and flips `live`, losers bail immediately and reload via the `live` watch. Only the winner computes a staleness key in this change-triggered path; the backstop and startup seed are separate per-instance paths that each compute one on their own cadence.

## Capabilities

### Modified Capabilities
- `mcp-orchestrated-indexing`: watcher always-on and is the primary freshness driver; hot-reload driven by the `live`-pointer event (filter exception, not the timer poll); `is_stale` from an event-seq/run-seq generation comparison; the read path never opens the store or touches git; cross-instance reload converges on the `live` event with non-blocking try-lock reindex.
- `workspace-staleness`: the git staleness key is computed over **tracked-modified files only** (never untracked), bounding its cost and removing untracked scratch from the digest.

## Impact

- **Latency**: read/status calls return in well under a second regardless of working-tree size — no git, no file hashing on the read path.
- **Behavior**: freshness reacts to actual file events (watcher) instead of a fixed interval; idle workspaces do zero git work.
- **Accepted limitations**: `is_stale` is change-seen (a revert to the indexed state still reads stale until the debounced reindex runs); the git backstop misses gitignored generated files (the watcher covers them).
- **Out of scope**: changing the reindex pipeline itself, the one-writer flock mechanism (reused as-is), or the non-git stat-tree digest form (kept for non-git workspaces).
- **Immediate mitigation already applied**: `node_modules/` added to `.gitignore` (untracked set 658 → 30, hashed bytes 26.5 MB → 0.11 MB) — this change makes the bound structural rather than incidental.
