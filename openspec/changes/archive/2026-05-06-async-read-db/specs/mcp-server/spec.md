## ADDED Requirements

### Requirement: Tool dispatch is async end-to-end

The MCP server SHALL dispatch tool calls through async functions all
the way down to the storage layer. Tools MUST NOT route through
`tokio::task::spawn_blocking` for the sole purpose of preventing a
nested tokio runtime — the storage layer is async-native and runs on
the caller's runtime.

The wire-level tool contract (input/output shapes, JSON-RPC error
codes including `INDEX_UNAVAILABLE`, pagination, and progress
notifications) is unchanged.

#### Scenario: Tool call does not consume a blocking-pool slot

- **GIVEN** the MCP server is in `Ready` state
- **WHEN** the agent issues a `tools/call` for any read tool
- **THEN** the tool body executes on the rmcp runtime's worker
  threads
- **AND** no `spawn_blocking` is involved in the storage path

#### Scenario: Wire contract is preserved

- **WHEN** an agent calls `get_workspace_overview` against a Ready
  server
- **THEN** the response payload is byte-for-byte equivalent to the
  pre-async version (same fields, same shapes)
- **AND** the same call against an `Indexing` server still returns
  `INDEX_UNAVAILABLE` with the same code and message form
