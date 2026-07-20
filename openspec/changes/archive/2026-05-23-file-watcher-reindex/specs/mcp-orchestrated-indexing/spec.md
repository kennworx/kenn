## ADDED Requirements

### Requirement: MCP server hosts an in-process file watcher

The `kenn mcp` server SHALL host an optional in-process file watcher
that observes the workspace root, filters events, debounces edit
bursts, and triggers a background reindex when the window closes. The
watcher SHALL be controllable from the agent via two tools and from
configuration via a boot-time flag.

The watcher pipeline SHALL be:

1. **Source:** filesystem events from the `notify` crate, rooted at
   the workspace, watched recursively. All event kinds (Create,
   Modify, Remove, Rename) contribute equally to the debounce window
   if they survive the filter — a deletion can invalidate symbols
   just as a save can add them.
2. **Filter (applied before debouncing):** an event survives only if
   the path matches a source extension (`Language::extensions()`) or a
   project file (`Language::project_files()`) for any indexed language,
   AND is not inside any directory in `WORKSPACE_SKIP_DIRS` (the shared
   constant used by the staleness fingerprint walk; includes `.git`
   and `.kenn`), AND does not match any user-supplied
   `[exclude] globs`.
3. **Debounce:** each surviving event resets a deadline of
   `mcp.watch_debounce_ms` (default 30000) into the future. When the
   deadline elapses with no further events, the watcher triggers once.
4. **Trigger:** the watcher calls `spawn_background_reindex`
   in-process. It does NOT bypass the staleness key or the
   one-writer flock.

The agent surface SHALL be:

- `watch_start` — idempotent. Starts the watcher; if one is already
  running, returns `WatchStartResult { started: false, debounce_ms }`
  without starting a second watcher.
- `watch_stop` — aborts the debounce task and drops the `notify`
  watcher. Permitted in any server state; calling it when no watcher
  is running is a no-op success.
- `get_index_status` — reports a `watcher` field with value
  `off` (not running), `idle` (running, no pending debounce), or
  `debouncing` (running, deadline pending).

`watch_start` SHALL error when the server is not in the `Ready`
state — there is no served snapshot to keep fresh during `Indexing`,
and `Failed` means no snapshot at all. The error message SHALL name
the current state. The agent's recovery is to poll `get_index_status`
until the state changes, then retry.

If `notify::RecommendedWatcher` initialization fails, `watch_start`
SHALL return an MCP error whose message includes the underlying
`notify` error, and SHALL leave the watcher state as `Off`.

When `mcp.watch_on = true`, the server SHALL start the watcher at
boot using the same code path as `watch_start`. If boot-time start
fails (e.g., `notify` initialization error), the server SHALL log a
warning and proceed to `Ready` without the watcher — a failure of an
optional convenience MUST NOT prevent the server from coming up.

`watch_on` is a boot-time default, not a supervisor: an explicit
`watch_stop` SHALL NOT auto-restart even when `watch_on = true`.

#### Scenario: Burst of saves collapses to one trigger

- **GIVEN** the watcher is running and `mcp.watch_debounce_ms = 30000`
- **WHEN** three source files are saved within 5 seconds and then no
  further events occur for 30 seconds
- **THEN** exactly one background reindex MUST be triggered

#### Scenario: Deletion of a source file triggers the debounce window

- **GIVEN** the watcher is running
- **WHEN** a tracked source file (`.cs`, `.rs`, etc.) is deleted
- **THEN** the deletion event MUST be treated like a save: it
  contributes to the debounce window
- **AND** at window expiry a background reindex MUST be triggered

#### Scenario: Build artifact under a default-skipped path is ignored

- **GIVEN** the watcher is running and no user `[exclude] globs` are
  configured
- **WHEN** a build process writes files into `target/` (or any other
  directory in `WORKSPACE_SKIP_DIRS`)
- **THEN** the events MUST be filtered before debouncing
- **AND** no background reindex MUST be triggered

#### Scenario: User-configured exclude glob is honored

- **GIVEN** the watcher is running and `[exclude] globs` contains `**/generated/**`
- **WHEN** a `.cs` file under `src/generated/` is written
- **THEN** the event MUST be filtered before debouncing
- **AND** no background reindex MUST be triggered

#### Scenario: `watch_start` is idempotent

- **GIVEN** the watcher is already running
- **WHEN** `watch_start` is called again
- **THEN** `WatchStartResult { started: false, debounce_ms }` MUST be returned
- **AND** no second watcher MUST be created

#### Scenario: `watch_stop` cancels a pending debounce

- **GIVEN** the watcher has a pending debounce deadline
- **WHEN** `watch_stop` is called before the deadline elapses
- **THEN** the pending trigger MUST be cancelled
- **AND** no reindex MUST be started

#### Scenario: `watch_stop` when no watcher is running is a no-op success

- **GIVEN** no watcher is running
- **WHEN** `watch_stop` is called
- **THEN** the call MUST succeed
- **AND** no error MUST be returned

#### Scenario: `watch_on` boots the watcher

- **GIVEN** `mcp.watch_on = true` in `kenn.toml`
- **WHEN** `kenn mcp` starts
- **THEN** the watcher MUST be running once the server reaches
  `Ready`
- **AND** `get_index_status` MUST report `watcher: "idle"` (or
  `"debouncing"` if an event has already landed)

#### Scenario: Explicit stop is not auto-restarted

- **GIVEN** `mcp.watch_on = true` and the watcher is running
- **WHEN** `watch_stop` is called
- **THEN** the watcher MUST remain stopped until `watch_start` is
  called again

#### Scenario: `watch_start` against a non-Ready server errors

- **GIVEN** the server is in `Indexing` (or `Failed`)
- **WHEN** `watch_start` is called
- **THEN** the call MUST return an MCP error whose message names the
  current state
- **AND** no watcher MUST be started

#### Scenario: `watch_on` with notify init failure brings up the server anyway

- **GIVEN** `mcp.watch_on = true` and the OS rejects `notify::Watcher`
  creation (e.g., inotify handle limit)
- **WHEN** `kenn mcp` starts
- **THEN** the server MUST log a warning describing the failure
- **AND** MUST proceed to `Ready` without the watcher
- **AND** `get_index_status` MUST report `watcher: "off"`

#### Scenario: Trigger respects the staleness key

- **WHEN** the debounce expires and the watcher triggers a reindex
- **AND** the workspace's staleness key matches the served snapshot's
  recorded key
- **THEN** the triggered reindex MUST be skipped by the staleness
  check
- **AND** no new snapshot MUST be published

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
