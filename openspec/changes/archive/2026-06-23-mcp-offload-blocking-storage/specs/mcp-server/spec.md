## MODIFIED Requirements

### Requirement: Tool dispatch is async end-to-end

The MCP server SHALL dispatch tool calls through async functions all the
way down to the storage layer. Tools MUST NOT route through
`tokio::task::spawn_blocking` for the sole purpose of preventing a nested
tokio runtime.

The storage **read** path SHALL execute its blocking SQLite work (connection
use and queries) on a dedicated per-snapshot connection pool, not on a
runtime worker thread. Concretely, the `Ready` snapshot binding SHALL hold a
read-only connection pool (opened once when the snapshot is bound) and tool
reads SHALL run their queries through it, so that (a) blocking SQLite never
occupies a runtime worker for the duration of the I/O, and (b) concurrent
reads proceed on separate connections rather than serializing behind a single
shared connection. The pool MUST NOT open a fresh connection per tool call on
the hot path.

The wire-level tool contract (input/output shapes, JSON-RPC error codes
including `INDEX_UNAVAILABLE` and `EMPTY_SNAPSHOT`, pagination, and progress
notifications) is unchanged.

#### Scenario: Blocking storage work does not occupy a runtime worker

- **GIVEN** the MCP server is in `Ready` state
- **WHEN** the agent issues a `tools/call` for any read tool
- **THEN** the tool's SQLite open/query runs on the snapshot pool's
  connection threads, not on the rmcp runtime's worker threads
- **AND** no `spawn_blocking` is involved in the storage path

#### Scenario: Concurrent reads do not serialize

- **GIVEN** the MCP server is in `Ready` state with a multi-connection pool
- **WHEN** several read `tools/call`s are in flight at once
- **THEN** they execute on separate pool connections in parallel
- **AND** one slow read does not block the others behind a single connection

#### Scenario: Wire contract is preserved

- **WHEN** an agent calls `get_workspace_overview` against a Ready server
- **THEN** the response payload is byte-for-byte equivalent to the
  pre-pool version (same fields, same shapes)
- **AND** the same call against an `Indexing` server still returns
  `INDEX_UNAVAILABLE` with the same code and message form
