## ADDED Requirements

### Requirement: MCP server owns its indexing lifecycle

The `kenn mcp` server SHALL inspect the workspace's snapshot state at
startup and run indexing in-process when the snapshot is missing or
stale. Indexing SHALL execute as a background task on the tokio
blocking thread pool so the MCP IO loop is never starved.

The lifecycle states are `Indexing`, `Ready`, and `Failed`. Transitions
SHALL be one-directional: `Indexing → Ready` on successful pipeline
completion, `Indexing → Failed` on pipeline error. There is no
automatic retry from `Failed`; the operator restarts the process or
runs `kenn index` manually.

#### Scenario: Cold start triggers indexing

- **WHEN** `kenn mcp <ws>` starts in a workspace with no `.kenn/live/`
- **THEN** the server enters the `Indexing` state immediately
- **AND** spawns a background task that calls `run_pipeline` against
  the workspace's `.kenn/building/`
- **AND** the MCP stdio transport is bound and accepting calls

#### Scenario: Fresh snapshot bypasses indexing

- **WHEN** `kenn mcp <ws>` starts in a workspace where `.kenn/live/`
  exists and the staleness check matches
- **THEN** the server transitions directly to `Ready` without running
  the pipeline

#### Scenario: Failed pipeline is terminal

- **WHEN** the background indexing task returns an error
- **THEN** the server transitions to `Failed` with the error message
  preserved
- **AND** the state remains `Failed` for the lifetime of the process

### Requirement: Snapshot freshness check reuses existing staleness machinery

The startup decision (run indexing vs. skip) SHALL use the
`compute_staleness_key` and `StalenessKey::matches` functions from
`kenn-store::staleness`. The decision SHALL honor the
`staleness.git_aware_skip` setting in `kenn.toml`.

When the staleness check itself fails (e.g. cannot read snapshot
metadata), the server SHALL conservatively re-index rather than serve
potentially-incorrect data.

#### Scenario: git_aware_skip true and key matches

- **GIVEN** `kenn.toml` sets `staleness.git_aware_skip = true`
- **AND** the workspace's current `StalenessKey` matches the key stored
  with the live snapshot
- **WHEN** the MCP server starts
- **THEN** the server transitions to `Ready` without indexing

#### Scenario: Staleness metadata unreadable

- **GIVEN** `.kenn/live/` exists but its staleness metadata cannot be
  parsed
- **WHEN** the MCP server starts
- **THEN** the server enters `Indexing` (treats unreadable metadata as
  stale)

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
