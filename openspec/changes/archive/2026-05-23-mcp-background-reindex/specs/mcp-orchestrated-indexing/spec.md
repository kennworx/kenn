## MODIFIED Requirements

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

## ADDED Requirements

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

While `Ready`, the MCP server SHALL detect when a newer snapshot has
been published to `.kenn/live/` — by `kenn index`, the file-watcher,
or its own background reindex — and SHALL atomically swap its
in-memory `Reader` to the newer snapshot without a process restart.

The swap SHALL be atomic with respect to in-flight tool calls: a call
that began against the old snapshot completes against it; calls that
begin after the swap use the new snapshot.

#### Scenario: External `kenn index` is picked up

- **GIVEN** the MCP server is `Ready`
- **WHEN** a separate `kenn index` run publishes a newer snapshot to
  `.kenn/live/`
- **THEN** the server swaps its reader to the new snapshot
- **AND** `get_index_status` reports the new `snapshot_id` and
  `indexed_at`

#### Scenario: In-flight calls are not disrupted by a swap

- **WHEN** the reader is swapped while a tool call is mid-execution
- **THEN** that call completes successfully against the snapshot it
  started on

#### Scenario: A snapshot that fails to open is not swapped in

- **WHEN** a newer snapshot is detected but opening a `Reader` against
  it fails (corrupt or partially published)
- **THEN** the server keeps serving the current snapshot
- **AND** it retries the swap on a later poll once the snapshot is
  valid

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

`get_index_status` SHALL report the true `is_stale` and
`reindex_in_progress` values rather than the current hard-coded
`false`. `is_stale` SHALL be `true` when the workspace's current
staleness key no longer matches the served snapshot's key.
`reindex_in_progress` SHALL be `true` while a background reindex is
running, and the status SHALL carry that reindex's progress snapshot
(phase and running counters) while it runs.

`get_index_status` SHALL return promptly and SHALL NOT perform git
operations on the call path; staleness MAY be evaluated on a
background cadence and the result cached.

#### Scenario: Stale snapshot is reported

- **GIVEN** the server is `Ready`
- **AND** workspace files have changed so the staleness key no longer
  matches the served snapshot
- **WHEN** `get_index_status` is called
- **THEN** `is_stale` is `true`

#### Scenario: In-progress reindex is reported

- **WHEN** `get_index_status` is called while a background reindex is
  running
- **THEN** `reindex_in_progress` is `true`
- **AND** the response carries the reindex's current phase and
  progress counters

#### Scenario: Idle ready server reports neither

- **WHEN** `get_index_status` is called on a `Ready` server with a
  fresh snapshot and no reindex running
- **THEN** `is_stale` and `reindex_in_progress` are both `false`

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

The MCP server SHALL support multiple `kenn mcp` processes — one per
Claude session — running concurrently against the same `.kenn/`
store. A server SHALL NOT assume it is the only process holding the
workspace: reindex coordination uses the cross-process one-writer
lock, snapshot GC honors cross-process reader pins, and hot-reload
lets every instance converge on the newest published snapshot. A
second instance starting against a workspace that another instance is
already serving SHALL NOT corrupt the store, block the first
instance, or fail to start.

#### Scenario: Second instance starts cleanly

- **GIVEN** one `kenn mcp` instance is already `Ready` on a workspace
- **WHEN** a second `kenn mcp` starts on the same workspace
- **THEN** the second instance reaches `Ready` independently
- **AND** neither instance's reads are disrupted

#### Scenario: All instances converge on the newest snapshot

- **GIVEN** several `kenn mcp` instances are `Ready` on one workspace
- **WHEN** any one of them — or a `kenn index` CLI run — publishes a
  newer snapshot
- **THEN** every instance hot-reloads to that snapshot
