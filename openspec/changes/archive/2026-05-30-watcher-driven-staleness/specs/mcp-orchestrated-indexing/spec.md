## MODIFIED Requirements

### Requirement: MCP server hosts an in-process file watcher

The `kenn mcp` server SHALL host an in-process file watcher that is the
**primary freshness driver**: it observes the workspace root, filters and
debounces edit bursts, and triggers a background reindex when the window
closes. The server SHALL **auto-start the watcher upon reaching `Ready`**
(default on) — it is no longer an opt-in convenience. `watch_stop` still
disables it for the session and `watch_start` re-enables it; both remain
agent-controllable, but the steady state is on.

The watcher pipeline SHALL be:

1. **Source:** `notify` events rooted at the workspace, watched recursively. All event kinds contribute to the debounce window.
2. **Filter:** an event survives only if the path matches a source extension (`Language::extensions()`) or a project file for any indexed language, AND is not inside any directory in `WORKSPACE_SKIP_DIRS` (includes `.git` and `.kenn`), AND does not match any user `[exclude] globs`. The filter consults file paths, **not git status**, so gitignored generated source files still trigger.
3. **Debounce:** each surviving event resets a `mcp.watch_debounce_ms` deadline; the watcher triggers once when it elapses.
4. **Trigger:** calls `spawn_background_reindex` in-process; it does NOT bypass the staleness key or the one-writer flock.

The recursive workspace watch already receives `.kenn/local/live`
events; they are dropped by the `WORKSPACE_SKIP_DIRS` filter today. The
watcher SHALL carve a **filter exception for exactly the `live` pointer**
(a symlink, retargeted on publish) so its events survive, while
**continuing to exclude the rest of `.kenn/`, including
`.kenn/local/runs/**`** — otherwise the indexer's own run writes would
re-trigger it. A surviving `live` event drives snapshot hot-reload (see
*Snapshot hot-reload*).

A **backstop** SHALL re-evaluate the staleness key on `spawn_blocking`
(never a dispatch worker) to cover OS events the watcher may drop,
triggering a reindex on mismatch. Its cadence SHALL be configurable
(`mcp.staleness_backstop_secs`, default 300; `0` disables). The backstop
relies on the git staleness key and therefore does not observe gitignored
generated files; those are covered by the watcher's path-based filter. It
also reconciles the startup window before the startup key-compare seed
lands.

`mcp.watch_on` SHALL default to `true` (the watcher is the primary
mechanism); an explicit `watch_stop` still disables it for the session,
after which only the backstop keeps the snapshot fresh.

#### Scenario: Watcher is on without an explicit start

- **WHEN** a `kenn mcp` server reaches `Ready`
- **THEN** `get_index_status.watcher` reports `idle` (or `debouncing`) without any `watch_start` call

#### Scenario: Burst of saves collapses to one trigger

- **GIVEN** the watcher is running and `mcp.watch_debounce_ms = 30000`
- **WHEN** three source files are saved within 5 seconds and then no further events occur for 30 seconds
- **THEN** exactly one background reindex MUST be triggered

#### Scenario: Backstop still reindexes after watch_stop

- **GIVEN** the watcher has been disabled via `watch_stop` (notify source torn down) and `mcp.staleness_backstop_secs > 0`
- **WHEN** a tracked source file changes and a backstop tick observes the key mismatch
- **THEN** the backstop increments `last_event_seq` AND triggers a reindex directly (not via the notify watcher)
- **AND** `is_stale` flips to `true` and the snapshot is eventually refreshed

#### Scenario: Gitignored generated source still triggers

- **WHEN** a gitignored but indexed source file (matching a watched extension, not under a skip dir) is written
- **THEN** the event survives the filter and contributes to the debounce window

### Requirement: Snapshot hot-reload

While `Ready`, the MCP server SHALL swap its in-memory `Reader` to a
newer published snapshot **driven by the watcher's `live`-pointer watch**
— not by a periodic timer probe of `live_target`. A change to the `live`
pointer (published by `kenn index`, the file-watcher reindex, its own
reindex, or another instance) SHALL trigger an atomic reader swap, with
self-publish dedup.

The swap SHALL be atomic with respect to in-flight tool calls: a call
that began against the old snapshot completes against it; calls that
begin after the swap use the new snapshot. A snapshot that fails to open
SHALL NOT be swapped in; the current snapshot keeps serving.

#### Scenario: External `kenn index` is picked up via the live watch

- **GIVEN** the MCP server is `Ready`
- **WHEN** a separate `kenn index` run flips the `live` pointer to a newer snapshot
- **THEN** the `live`-pointer watch fires and the server swaps its reader, with no timer poll involved
- **AND** `get_index_status` reports the new `snapshot_id` and `indexed_at`

#### Scenario: In-flight calls are not disrupted by a swap

- **WHEN** the reader is swapped while a tool call is mid-execution
- **THEN** that call completes successfully against the snapshot it started on

#### Scenario: Self-publish does not loop

- **WHEN** this instance's own reindex flips `live`
- **THEN** the resulting `live` event is a no-op (`resolved(live) == current`) and does not re-trigger work

### Requirement: Index status reports staleness and reindex progress

`get_index_status` SHALL report `is_stale` and `reindex_in_progress`,
and SHALL return promptly performing **no git operations and no store
open** on the call path (read path = current run + cached state only).

`is_stale` SHALL be derived from a **generation comparison**, not a
set/clear bool: a monotonic per-process `last_event_seq` (incremented
when this instance's watcher observes a source event) versus the served
run's `run_event_seq`. `is_stale = last_event_seq > run_event_seq`. This
avoids losing a change that lands mid-reindex (a bool cleared on publish
would mask it).

`run_event_seq` SHALL be **in-memory only** (held alongside the reader
binding, never persisted per run). It is updated on a reader swap,
**split by provenance** because `last_event_seq` is process-local:

- **Self-publish swap** (the reindex-completion path): the triggering
  reindex captures this instance's `last_event_seq` at its **start**,
  holds it, and applies it to the in-memory `run_event_seq` atomically
  with the reader swap. Any later event keeps `is_stale` true. The
  instance's own `live` event is a no-op via the self-dedup, so it does
  not also take the cross-instance path.
- **Cross-instance swap** (the `live`-watch path, after self-dedup):
  there is no commensurable counter to read, so the swap SHALL set
  `run_event_seq := last_event_seq` snapshot at reload. A change that
  raced the cross-instance publish is reconciled by the backstop, not the
  counter (accepted limitation).

The **only** per-run persisted freshness artifact is the staleness key
(below); no on-disk `run_event_seq` is read by any path.

The startup seed and the backstop compute a **git staleness key**, not a
counter; a key-compare reporting stale SHALL **synthesize an event** so
the single comparison above stays authoritative. "Synthesize an event"
names two required effects, identical to a real fs event: it both
increments `last_event_seq` (so `is_stale` flips) **and** drives a
debounced reindex. A bare atomic bump is insufficient — it would flip
`is_stale` without ever reindexing, defeating the backstop. The seed and
backstop SHALL achieve both effects by invoking the reindex trigger
directly, independently of the notify watcher's liveness, so the backstop
still reindexes after `watch_stop` has torn the watcher down. This requires each run to record the
staleness key it was built against. On reaching `Ready` the initial open
initializes `run_event_seq := last_event_seq` (same as a cross-instance
swap), and the server SHALL run one background `spawn_blocking`
key-compare against the served run (off the call path); until it lands,
`is_stale` is optimistically `false`.
Semantics are *change-seen since the run's start* (not content-differs);
a revert to the indexed state reads stale until reindex. No git runs on
the call path. `reindex_in_progress` SHALL be `true` while a background
reindex runs, with the progress snapshot carried alongside.

#### Scenario: Stale is reported without git on the call path

- **GIVEN** the server is `Ready` and the watcher has observed a source change since the served run
- **WHEN** `get_index_status` is called
- **THEN** `is_stale` is `true`
- **AND** the call performs no git subprocess and no `Store::open`

#### Scenario: Large untracked tree does not slow status

- **GIVEN** a working tree with a large untracked directory
- **WHEN** `get_index_status` (or any read tool) is called
- **THEN** it returns in well under a second, hashing nothing

### Requirement: Multiple MCP instances share one workspace store

Multiple `kenn mcp` processes SHALL run concurrently against one `.kenn/`
store without corruption, blocking, or startup failure. On a workspace
change, each instance's watcher SHALL attempt reindex via a
**non-blocking try-lock** on the one-writer flock: the winner reindexes
and flips `live`; losers bail immediately (no blocking wait) and reload
via their `live`-pointer watch when the winner publishes. Only the
winner computes a staleness key **in this change-triggered reindex
path**; the backstop and startup seed are separate per-instance paths
that each compute one on their own cadence. Snapshot GC continues to
honor cross-process reader pins.

#### Scenario: One change, one reindex, all reload

- **GIVEN** several `kenn mcp` instances are `Ready` on one workspace
- **WHEN** a source change occurs that all watchers observe
- **THEN** exactly one instance acquires the one-writer lock and reindexes
- **AND** the others do not block on the lock
- **AND** every instance hot-reloads to the published snapshot via its `live` watch

#### Scenario: Reloading instance reports fresh, then re-detects local changes

- **GIVEN** instance B hot-reloads a run that instance A published (cross-instance swap)
- **THEN** B sets `run_event_seq` from its own `last_event_seq` snapshot, so `is_stale` reads `false` immediately after the reload
- **AND WHEN** B's watcher subsequently observes a new source event
- **THEN** `is_stale` reads `true` for B (it does not stay masked by an incommensurable foreign counter)

#### Scenario: Second instance starts cleanly

- **GIVEN** one `kenn mcp` instance is already `Ready`
- **WHEN** a second starts on the same workspace
- **THEN** it reaches `Ready` independently and neither instance's reads are disrupted
