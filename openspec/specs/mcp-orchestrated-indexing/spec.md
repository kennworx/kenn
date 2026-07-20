# mcp-orchestrated-indexing

## Purpose

The `kenn mcp` server owns its indexing lifecycle: it inspects the
workspace at startup, runs indexing when the snapshot is missing or
stale, and surfaces progress to the agent. The pipeline runs on a
tokio blocking thread; everything around it is async.
## Requirements
### Requirement: MCP server owns its indexing lifecycle

The `kenn mcp` server SHALL inspect the workspace's snapshot state at
startup and run indexing on the rmcp runtime when the snapshot is
missing or stale. The pipeline body itself runs on a tokio blocking
thread (it is CPU-bound), but startup decision, snapshot opening,
and lifecycle transitions are async-native and run on the rmcp
runtime — they do not require a separate blocking thread.

The lifecycle states are `Indexing`, `Ready`, and `Failed`. The
**cold-start** path is one-directional: `Indexing → Ready` on
successful pipeline completion, `Indexing → Failed` on pipeline error.
There is no *automatic* retry from `Failed`, but the `reindex` tool
MAY be invoked against a `Failed` server to retry (`Failed →
Indexing`); the operator can also restart the process or run
`kenn index` manually.

Once `Ready`, the server MAY re-enter indexing for a **background
reindex** — `Ready` is NOT terminal. During a background reindex the
server SHALL continue to serve reads from the current snapshot, and a
background-reindex failure SHALL return the server to the prior
`Ready` snapshot rather than transitioning to `Failed`.

#### Scenario: Cold start triggers indexing

- **WHEN** `kenn mcp <ws>` starts in a workspace with no `.kenn/live/`
- **THEN** the server enters the `Indexing` state immediately
- **AND** spawns a tokio task that drives the indexing workflow
- **AND** the MCP stdio transport is bound and accepting calls

#### Scenario: Fresh snapshot bypasses indexing

- **WHEN** `kenn mcp <ws>` starts in a workspace where `.kenn/live/`
  exists and the staleness check matches
- **THEN** the server transitions directly to `Ready` without running
  the pipeline

#### Scenario: Failed cold-start pipeline halts the server

- **WHEN** the startup indexing task returns an error
- **THEN** the server transitions to `Failed` with the error message
  preserved
- **AND** the state remains `Failed` until a `reindex` call or a
  process restart

#### Scenario: Background reindex does not enter Failed

- **GIVEN** the server is `Ready`
- **WHEN** a background reindex it started returns an error
- **THEN** the server remains `Ready` on the pre-reindex snapshot
- **AND** the server does NOT transition to `Failed`

### Requirement: Snapshot freshness check reuses existing staleness machinery

The startup decision (run indexing vs. skip) SHALL use the
`compute_staleness_key` and `StalenessKey::matches` functions from
`kenn-store::staleness`. The decision SHALL honor the
`staleness.git_aware_skip` setting in `kenn.toml`.

The decision SHALL consider every retained snapshot under the derived
store, not only the one `live` points at: the server SHALL select the
retained snapshot whose recorded `StalenessKey` matches the
workspace's current key, and SHALL skip indexing only when such a
snapshot exists. When no retained snapshot matches, the server SHALL
re-index. This lets a derived store shared across branches or
worktrees serve each from its own matching snapshot.

This SHALL apply uniformly whether or not the workspace is a git
repository. A non-git workspace carries a tree-fingerprint
`StalenessKey` (see the `workspace-staleness` capability), so the
startup decision SHALL resolve it through the same scan-by-key path it
uses for a git workspace — it SHALL NOT special-case a non-git
workspace. (This supersedes the interim non-git "serve `live`" degrade
from `config-driven-store-layout`, which existed only because non-git
workspaces previously had no usable key.)

When the staleness check itself fails (e.g. cannot read snapshot
metadata, or the workspace fingerprint cannot be computed), the server
SHALL conservatively re-index rather than serve potentially-incorrect
data.

#### Scenario: git_aware_skip true and a retained snapshot matches

- **GIVEN** `kenn.toml` sets `staleness.git_aware_skip = true`
- **AND** the workspace's current `StalenessKey` matches the key
  recorded with some retained snapshot
- **WHEN** the MCP server starts
- **THEN** the server opens that snapshot and transitions to `Ready`
  without indexing

#### Scenario: non-git workspace resolves by tree fingerprint

- **GIVEN** `staleness.git_aware_skip = true`
- **AND** a non-git workspace whose tree-fingerprint key matches the key
  recorded with some retained snapshot
- **WHEN** the MCP server starts
- **THEN** the server opens that snapshot and transitions to `Ready`
  without indexing

#### Scenario: non-git workspace changed since its last index

- **GIVEN** a non-git workspace whose source tree has changed since the
  retained snapshot was built
- **WHEN** the MCP server starts
- **THEN** no retained snapshot's tree fingerprint matches, and the
  server re-indexes

#### Scenario: staleness metadata unreadable

- **GIVEN** a retained snapshot exists but its staleness metadata
  cannot be parsed
- **WHEN** the MCP server starts
- **THEN** the server re-indexes (treats unreadable metadata as stale)

### Requirement: Pipeline emits progress events through a callback

`run_pipeline` SHALL accept an optional progress callback parameter of
type `impl Fn(ProgressEvent) + Send + Sync`. The callback SHALL be
invoked at well-defined points in the pipeline:

- Once at the start (`ProgressEvent::Started`).
- At each phase boundary (`ProgressEvent::Phase`,
  `ProgressEvent::PhaseEnd`).
- At each batch flush (`ProgressEvent::Batch` carrying running
  record/file/symbol counters).
- Once at the end (`ProgressEvent::Completed`).

CLI callers (`cmd_index.rs`) SHALL continue to function with a no-op
callback — the callback is purely additive.

#### Scenario: CLI passes a no-op callback

- **WHEN** `cmd_index.rs` invokes `run_pipeline` with `|_| {}` as
  the progress argument
- **THEN** the pipeline runs identically to its prior behavior
- **AND** no events are forwarded anywhere

#### Scenario: MCP forwards events to rmcp notifications

- **WHEN** the MCP server's background indexing task receives a
  `ProgressEvent::Batch { files, symbols, .. }`
- **THEN** the server emits an rmcp `notifications/message` at info
  level with a human-readable progress string
- **AND** the same event updates the cached progress snapshot inside
  `LifecycleState::Indexing`

### Requirement: SurrealdbSink supports optional FULLTEXT defer

The `SurrealdbSink` SHALL accept a `SinkOptions { defer_fulltext: bool }`
parameter (default `false`). When the flag is `false`, the sink applies
both `SCHEMA_SURQL` and `FULLTEXT_SURQL` at `begin_run` and `end_run` is
a no-op for indexes (current behavior). When the flag is `true`, the
sink applies `SCHEMA_SURQL` at `begin_run` and `FULLTEXT_SURQL` at
`end_run`, deferring FULLTEXT BM25 index construction until after bulk
ingest.

The default `false` value SHALL preserve the unchanged behavior of
`kenn index`. Callers that want the deferral (e.g. MCP under phase-
gated tools) opt in explicitly.

#### Scenario: Default — FULLTEXT inline

- **WHEN** `SurrealdbSink::create(dir)` is called without options
- **THEN** the sink applies the full schema (including FULLTEXT
  indexes) at `begin_run`
- **AND** index maintenance happens incrementally per insert
- **AND** `end_run` does not apply additional schema

#### Scenario: Defer flag set — FULLTEXT at end_run

- **WHEN** `SurrealdbSink::create_with_options(dir, SinkOptions {
  defer_fulltext: true })` is called
- **THEN** `begin_run` applies only `SCHEMA_SURQL` (no FULLTEXT)
- **AND** `end_run` applies `FULLTEXT_SURQL` against the populated
  tables
- **AND** the resulting database has both BM25 indexes available

### Requirement: Storage layer exposes async API

The storage layer SHALL expose async functions on
`kenn_store::db::ReadDb` and `kenn_store::db::SurrealdbSink::create`
that run on the caller's tokio runtime.
They MUST NOT own a private `tokio::runtime::Runtime` for the purpose
of bridging to async query libraries. The Sink trait
(`begin_run`/`write_batch`/`end_run`) stays synchronous; its
implementation MAY use `tokio::runtime::Handle::current().block_on`
when invoked from a sync caller that is itself running inside a
tokio blocking thread (the pipeline path).

The `ServerState::bootstrap_blocking` synchronous helper SHALL be
removed. Callers that need to populate state from disk MUST `.await`
the async equivalent from a tokio context.

#### Scenario: ReadDb method is async

- **WHEN** a tool calls `ReadDb::fetch_symbol`
- **THEN** the call awaits the SurrealDB query on the caller's runtime
- **AND** no nested runtime is created

#### Scenario: bootstrap_blocking removed

- **WHEN** the `kenn-mcp` crate is searched for `bootstrap_blocking`
- **THEN** the symbol does not exist
- **AND** the integration tests cover the same scenarios via an
  `async fn bootstrap` invoked from `#[tokio::test]` contexts

### Requirement: Analysis phase in the index pipeline

The `kenn-indexer` `workflow::index_workspace` SHALL run an analysis phase after the aggregate-graph phase. The analysis phase SHALL:

1. Load the just-written aggregate graph in-process.
2. Call `kenn_analyze::compute_analysis(&graph, &opts)` to produce an `AnalysisResult`.
3. Persist the result via the new `Writer::write_analysis(&AnalysisResult)` method.
4. When `[index] write_report = true` (default), call `kenn_analyze::render_report(&graph, &result)` and write the output to `<workspace>/kenn-out/REPORT.md`.

The phase SHALL be gated by `[index] persist_analysis` (default `true`). When false, neither the analysis tables nor REPORT.md are written, regardless of `[index] write_report`.

The phase SHALL emit `ProgressEvent::PhaseStarted("analysis")` and `ProgressEvent::PhaseFinished("analysis")` so MCP's orchestrated-indexing status surface reflects it the same way it reflects the existing phases.

#### Scenario: Analysis phase runs after aggregation

- **WHEN** `kenn index` runs successfully against any workspace with `[index] persist_analysis = true`
- **THEN** the indexer MUST emit `PhaseStarted("analysis")` and `PhaseFinished("analysis")` events
- **AND** the resulting snapshot MUST contain populated `analysis_*` tables
- **AND** the analysis phase MUST occur after the aggregation phase and before the snapshot commit

#### Scenario: Analysis phase skipped when disabled

- **WHEN** `kenn.toml` contains `[index] persist_analysis = false`
- **AND** `kenn index` runs successfully
- **THEN** the indexer MUST NOT emit `PhaseStarted("analysis")`
- **AND** the snapshot's `analysis_*` tables MUST be empty (or absent)
- **AND** `kenn-out/REPORT.md` MUST NOT be created or modified by this run

### Requirement: Analysis options surfaced through `[index]`

The `kenn_analyze::AnalysisOptions` knobs that previously rode on `kenn analyze` CLI flags SHALL be configurable under `[index]`:

- `[index] analysis.top_n` (default `20`) — top-N for each god-node list.
- `[index] analysis.max_depth` (default `4`) — maximum hierarchy depth.
- `[index] analysis.min_cluster` (default `20`) — minimum community size to recurse into.

The workflow SHALL read these values when constructing `AnalysisOptions` for the analysis phase.

#### Scenario: Analysis knobs respected at index time

- **WHEN** `kenn.toml` contains `[index] analysis = { top_n = 50, max_depth = 6, min_cluster = 10 }`
- **AND** `kenn index` runs
- **THEN** the persisted `analysis_god_nodes` table MUST contain up to 50 rows per filter
- **AND** the persisted `analysis_anchored_hierarchy` tree MUST contain communities at depth 6 where the source data permits

### Requirement: Background reindex tool

The MCP server SHALL expose a `reindex` tool that triggers an
in-process reindex of the workspace. The reindex SHALL run in the
background: the tool call returns promptly without blocking until
indexing completes, and the server SHALL keep serving reads from the
current `Ready` snapshot for the whole duration. On successful
completion the server SHALL atomically swap its reader to the new
snapshot.

At most one reindex SHALL run against a workspace at a time, across
all processes. Reindex starts SHALL be serialized through the store's
existing one-writer lock (`index.lock`): a `reindex` call — or a
cold-start reindex — that cannot acquire the lock because this or
another `kenn mcp` instance, or a `kenn index` CLI run, already holds
it SHALL NOT error. It SHALL report the in-progress run and rely on
snapshot hot-reload to pick up the result. A `reindex` call received
while the server is still in cold-start `Indexing` SHALL likewise be a
no-op that reports the in-progress run.

When invoked against a `Failed` server, `reindex` instead acts as a
recovery retry: the server SHALL transition `Failed → Indexing` and
run the pipeline as at cold start. There is no current snapshot to
serve, so non-status tools return `INDEX_UNAVAILABLE` until the retry
reaches `Ready`.

#### Scenario: Reindex runs without blocking reads

- **GIVEN** the server is `Ready`
- **WHEN** the `reindex` tool is called
- **THEN** the call returns promptly
- **AND** a background indexing run starts
- **AND** other tool calls continue to be served from the current
  snapshot while it runs

#### Scenario: Concurrent reindex is coalesced

- **WHEN** `reindex` is called while a background reindex is already
  running
- **THEN** no second indexing run starts
- **AND** the response reports the already-in-progress run

#### Scenario: Reindex coalesced across instances

- **GIVEN** two `kenn mcp` instances are running on the same workspace
- **WHEN** `reindex` is called on the second instance while the first
  instance is mid-reindex (it holds `index.lock`)
- **THEN** the second instance does not start a competing indexing run
- **AND** the call does not error
- **AND** the second instance hot-reloads the snapshot the first
  instance publishes

#### Scenario: Successful reindex swaps to the new snapshot

- **WHEN** a background reindex completes successfully
- **THEN** the server atomically swaps its reader to the new snapshot
- **AND** subsequent tool calls are served from the new snapshot

#### Scenario: Reindex during cold-start indexing

- **WHEN** `reindex` is called while the server is still in cold-start
  `Indexing`
- **THEN** no second indexing run starts
- **AND** the call reports the in-progress cold-start run

#### Scenario: Reindex recovers a Failed server

- **GIVEN** the server is in `Failed` after a cold-start pipeline error
- **WHEN** the `reindex` tool is called
- **THEN** the server transitions `Failed → Indexing` and retries the
  pipeline
- **AND** it reaches `Ready` if the retry succeeds

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

### Requirement: Embedding refresh is coordinated cross-process

The MCP server SHALL ensure a served snapshot's embedding vectors are
populated. A snapshot produced by `kenn index` carries null embeddings
— the structural index defers embedding to a separate job. The server
SHALL trigger that job both after a cold-start `Ready` and after every
hot-reload swap onto a new snapshot.

Every embed trigger SHALL be coordinated cross-process, and the
coordination SHALL live in the embed job itself so that all call sites
— cold-start, hot-reload, and the `kenn embed` CLI — are covered
uniformly. At most one embed run SHALL execute against a given
snapshot at a time, serialized through a lock. A trigger that cannot
acquire the lock SHALL skip its own run — another process is already
embedding that snapshot, and because the embed job republishes the
store in place, every instance reader-bound to that snapshot observes
the vectors appear with no reopen. A trigger SHALL also skip when the
snapshot is already fully embedded. This prevents N instances that
cold-start or hot-reload onto the same snapshot from each running the
expensive embedding inference redundantly.

#### Scenario: Embed runs once after hot-reloading a CLI snapshot

- **GIVEN** the server hot-reloads onto a snapshot produced by
  `kenn index` whose embeddings are null
- **AND** no other process is embedding that snapshot
- **THEN** the server runs the embed job for that snapshot
- **AND** vector-search coverage is restored once the job completes

#### Scenario: Embed coordinated across instances at cold start

- **GIVEN** several `kenn mcp` instances cold-start on the same
  workspace and reach `Ready` on a null-embedding snapshot
- **WHEN** each instance triggers the embed job
- **THEN** exactly one embed run executes against that snapshot
- **AND** the instances that do not acquire the lock skip their run

#### Scenario: Concurrent embed is skipped, not duplicated

- **GIVEN** two instances would embed the same null-embedding snapshot
- **WHEN** the first instance acquires the embed lock and starts
  embedding
- **THEN** the second instance does not start a competing embed run
- **AND** the second instance observes the vectors in place as the
  first instance republishes them

#### Scenario: Already-embedded snapshot is skipped

- **WHEN** a snapshot that is already fully embedded would be embedded
- **THEN** no embed run is started

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

### Requirement: A reader-held snapshot is pinned against GC

Snapshot garbage collection SHALL retain any snapshot directory that a
`kenn mcp` instance's `Reader` currently holds open, even when newer
snapshots would otherwise evict it under `[lifecycle] gc_keep`. The
pin SHALL be cross-process: a snapshot held by one instance SHALL
survive a GC sweep run by a different instance or by a `kenn index`
CLI run. The pin SHALL be released once the reader is swapped away
from that snapshot, and SHALL auto-release if the holding process
exits or crashes. Pins SHALL be tracked by a store-level `flock`
reader registry (`.kenn/local/readers/`) that GC probes non-blocking,
so published snapshot directories stay immutable.

An in-flight `embed_pending` job SHALL hold a pin in the same registry
for its target snapshot for the duration of the embed, and the pin
SHALL drop when the embed job returns. Because `embed_pending` no
longer holds the store-level `index.lock` (it uses a per-snapshot
embed lock instead — see "Embedding refresh is coordinated
cross-process"), a concurrent reindex could otherwise publish a newer
snapshot, the server hot-reload onto it, the prior MCP reader-pin
drop, and a third instance's GC evict the snapshot mid-write. The
embed-side pin closes that race.

#### Scenario: Held snapshot survives GC

- **GIVEN** an MCP server `Ready` on snapshot `S`
- **WHEN** enough newer snapshots are published that `gc_keep` would
  normally evict `S`
- **THEN** `S` is retained on disk for as long as the server's reader
  holds it

#### Scenario: Snapshot held by another instance survives GC

- **GIVEN** instance A is `Ready` on snapshot `S`
- **WHEN** instance B finishes a reindex and runs GC that would
  otherwise evict `S`
- **THEN** B's GC skips `S` because A still holds it
- **AND** `S` is collected only once no instance holds it

#### Scenario: Pin released on process exit

- **GIVEN** an instance holds snapshot `S` and then exits or crashes
- **WHEN** a GC sweep next runs
- **THEN** `S`'s reader marker no longer pins it
- **AND** `S` is eligible for collection under normal `gc_keep` rules

#### Scenario: Pin released after swap

- **WHEN** the server swaps its reader off snapshot `S` to a newer one
- **THEN** `S` becomes eligible for garbage collection again

#### Scenario: Snapshot survives GC while embed is in flight

- **GIVEN** an `embed_pending` job is mid-write to snapshot `S`
- **WHEN** a GC sweep runs that would otherwise evict `S`
- **THEN** `S` is retained until the embed job completes
- **AND** `S` is eligible for collection once the embed-side pin drops

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

### Requirement: Code-update notification on every reader swap

The server SHALL emit an MCP `notifications/message` on every successful `ArcSwap` swap performed by the snapshot poll task. The notification's `level` SHALL be `"info"` and its `data` SHALL contain at minimum `event: "code_updated"` and a human-readable `message`.

The event name is consumer-oriented: from the agent's perspective the
relevant fact is that the indexed code it queries against has been
updated. The notification SHALL be emitted on every successful swap
regardless of who caused the new snapshot — the in-process watcher,
the `reindex` tool, an external `kenn index` run, or another
`kenn mcp` instance's background reindex. This gives the agent a
single, source-agnostic signal to re-query if it cares about freshness.

When no MCP client is connected, the notification SHALL be dropped at
the existing peer-gone handling path (logged at `debug`) and SHALL
NOT block the swap.

#### Scenario: External `kenn index` produces a code-update notification

- **GIVEN** the MCP server is `Ready` with an MCP client connected
- **WHEN** a separate `kenn index` run publishes a newer snapshot
- **AND** the poll task swaps the reader to the new snapshot
- **THEN** the client MUST receive a `notifications/message` with
  `data.event = "code_updated"`

#### Scenario: Watcher-triggered reindex produces a code-update notification

- **GIVEN** the in-process watcher triggers a background reindex that
  publishes a newer snapshot
- **WHEN** the poll task swaps the reader
- **THEN** the same `code_updated` notification MUST be observed —
  using the same notification shape as an external reindex

#### Scenario: No connected client is not an error

- **GIVEN** no MCP client is connected
- **WHEN** the poll task swaps the reader
- **THEN** the notification MUST be dropped gracefully (debug-logged)
- **AND** the swap MUST complete normally

### Requirement: Schema-version mismatch routes through Failed for recovery

When the snapshot-open path returns a `SchemaMismatch` error (per `store-layout`'s `Snapshots carry a store-schema version` requirement), the MCP server SHALL map it to `LifecycleState::Failed` with an `error` string naming both the persisted snapshot version and the binary's expected `STORE_SCHEMA_VERSION`, and the `reindex required` action.

Because `LifecycleState::Failed` already has an established recovery path (`spawn_recovery_pipeline` on the next `reindex` tool call OR on the next staleness re-check), schema-mismatch SHALL converge to `Ready` through that machinery without a new lifecycle state. `get_index_status` reports the standard `failed` state with the schema-mismatch text in its `error` field — no new wire shape.

Schema-mismatch SHALL NOT cause the server process to exit. Reads return `INDEX_UNAVAILABLE` while the recovery reindex runs, matching the existing Failed-state behavior.

#### Scenario: A schema-mismatched snapshot is reported as failed, then recovered

- **GIVEN** the workspace's only retained snapshot persists `schema_version = 1`
- **AND** the running binary's `STORE_SCHEMA_VERSION` is `2`
- **WHEN** the MCP server starts
- **THEN** the lifecycle transitions to `Failed` with an `error` string naming both versions
- **AND** `get_index_status` returns `{state: "failed", error: "schema v1, binary expects v2; reindex required"}`
- **AND** read tools return `INDEX_UNAVAILABLE`

#### Scenario: The reindex tool recovers a schema-stale state

- **GIVEN** the server is in `Failed` due to a schema-mismatch
- **WHEN** the agent calls the `reindex` tool
- **THEN** the server transitions to `Indexing` and runs the pipeline
- **AND** the new snapshot persists the current `STORE_SCHEMA_VERSION`
- **AND** the next snapshot open succeeds and the server reaches `Ready`

### Requirement: Cold start does not serve an empty snapshot for a configured workspace

The cold-start startup decision SHALL NOT skip to (serve as `Ready`) a
retained snapshot that contains **zero symbols** when the active
`kenn.toml` has **at least one language enabled**. In that case the
server SHALL re-index instead — it remains in `Indexing` (data tools
fail fast with `INDEX_UNAVAILABLE`) until the re-index completes, rather
than presenting an empty `Ready` snapshot that an agent would misread as
"the index is not built."

This refines the snapshot-freshness skip rule: a matching `StalenessKey`
is necessary but no longer sufficient to skip — a zero-symbol snapshot
under a language-enabled config is treated as not serviceable. This
recovers the common case where a prior index run produced zero symbols
because of a transient indexer failure (a language server failed to
launch) and published the empty result under the workspace's key: the
next cold start re-indexes rather than serving the stale empty snapshot
indefinitely.

A workspace that legitimately yields no symbols SHALL still settle to
`Ready` and SHALL NOT cause a per-startup re-index loop:

- When no `kenn.toml` exists, or every `[language.*].enabled` is false,
  the config does not expect symbols; the server SHALL settle to `Ready`
  on the empty snapshot and surface the existing empty-snapshot
  config-hint (`not-initialized` / `config-disabled`).
- When at least one language is enabled but the re-index again produces
  zero symbols, the server SHALL settle to `Ready` on that freshly-built
  empty snapshot and surface the `configured-but-empty` hint. The
  re-index runs at most once per cold start; the server does not loop.

#### Scenario: Empty snapshot under enabled language triggers re-index

- **GIVEN** a retained snapshot whose `StalenessKey` matches the
  workspace but which contains zero symbols
- **AND** `kenn.toml` enables at least one language
- **WHEN** the MCP server starts
- **THEN** the server does NOT serve the empty snapshot as `Ready`
- **AND** the server enters `Indexing` and re-runs the pipeline

#### Scenario: Re-index now produces symbols

- **GIVEN** the empty-snapshot re-index path is taken
- **AND** the indexer now succeeds (the prior emptiness was a transient
  failure)
- **WHEN** the re-index completes
- **THEN** the server transitions to `Ready` on a populated snapshot

#### Scenario: Genuinely empty configured workspace settles without looping

- **GIVEN** a workspace with a language enabled but no matching source
  files
- **WHEN** the MCP server starts and the cold-start re-index produces an
  empty snapshot
- **THEN** the server settles to `Ready` on that snapshot with the
  `configured-but-empty` config-hint
- **AND** the server does NOT immediately re-index again

#### Scenario: Unconfigured workspace settles to Ready without re-index

- **GIVEN** a workspace with no `kenn.toml` (or all languages disabled)
- **WHEN** the MCP server starts with an empty live snapshot
- **THEN** the server settles to `Ready` and surfaces the
  `not-initialized` / `config-disabled` hint
- **AND** the server does NOT trigger a re-index on account of the empty
  snapshot

### Requirement: Index status reports the served snapshot's degraded-run summary

`get_index_status` SHALL report whether the **served snapshot** was built
from a degraded run (`wait_for_index` returns the same payload):
the aggregate run status recorded in the snapshot's metadata
(`"success" | "partial" | "failed"`), the bounded failed-project attribution
list with its true total count, and the bounded status-neutral warning list
with its true total count. When the run was clean — `success` with no
warnings — these fields SHALL be omitted, leaving the payload unchanged.

The summary SHALL be parsed from the snapshot's persisted metadata **once per
reader binding** (cold start, recovery, and every snapshot rotation) and
served from that cached state — never a store open or metadata read on the
status call path. A snapshot without parseable metadata (pre-reporting era)
SHALL yield no summary, not an error.

Degradation SHALL be reported, not escalated: a `partial` snapshot still
serves, and the `state` field continues to reflect the lifecycle/embed stage.

#### Scenario: a partial run's failures are visible to the agent

- **GIVEN** an index run where one language sidecar failed (e.g. C# msbuild)
- **WHEN** the resulting snapshot is served and `get_index_status` is called
- **THEN** the payload carries `run_status: "partial"` and the failed-project
  attribution naming that language
- **AND** `failed_count` is the true total (bounded list length + overflow)
- **AND** `state` still reflects the embed stage (the graph serves)

#### Scenario: producer warnings surface without changing the state

- **GIVEN** a successful run that recorded status-neutral warnings (e.g.
  stale index-store units kept on a trusted read)
- **WHEN** `get_index_status` is called
- **THEN** the payload carries the warning list and `warning_count`
- **AND** `run_status` is `"success"`

#### Scenario: a clean run leaves the payload unchanged

- **GIVEN** a snapshot whose run succeeded with no warnings
- **WHEN** `get_index_status` is called
- **THEN** none of the degraded-run fields are present

#### Scenario: the summary tracks snapshot rotation from cached state

- **GIVEN** a served `partial` snapshot and a subsequent clean reindex
- **WHEN** the `live` pointer flips and the reader swaps
- **THEN** the next `get_index_status` reflects the new snapshot's clean
  summary
- **AND** no metadata read happens on the status call path itself

