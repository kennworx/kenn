## MODIFIED Requirements

### Requirement: MCP server owns its indexing lifecycle

The `kenn mcp` server SHALL inspect the workspace's snapshot state at
startup and run indexing on the rmcp runtime when the snapshot is
missing or stale. The pipeline body itself runs on a tokio blocking
thread (it is CPU-bound), but startup decision, snapshot opening,
and lifecycle transitions are async-native and run on the rmcp
runtime — they do not require a separate blocking thread.

The lifecycle states are `Indexing`, `Ready`, and `Failed`.
Transitions SHALL be one-directional: `Indexing → Ready` on
successful pipeline completion, `Indexing → Failed` on pipeline
error. There is no automatic retry from `Failed`; the operator
restarts the process or runs `kenn index` manually.

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

#### Scenario: Failed pipeline is terminal

- **WHEN** the indexing task returns an error
- **THEN** the server transitions to `Failed` with the error message
  preserved
- **AND** the state remains `Failed` for the lifetime of the process

## ADDED Requirements

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
