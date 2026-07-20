## ADDED Requirements

### Requirement: Server binds stdio without a pre-existing snapshot

The MCP server SHALL bind its stdio transport and accept tool calls
immediately on launch, regardless of whether `.kenn/live/` exists.
Indexing, if needed, runs in the background; it does not block the
transport bind.

#### Scenario: Launch in unindexed workspace

- **WHEN** `kenn mcp <ws>` is launched in a workspace with no
  `.kenn/live/`
- **THEN** the MCP server completes its rmcp `initialize` handshake
  within typical handshake latency (no blocking on indexing)
- **AND** subsequent `tools/list` calls return the full tool list
- **AND** `tools/call get_index_status` returns the current lifecycle
  state

### Requirement: Tools other than `get_index_status` fail fast while not Ready

The MCP server SHALL return a JSON-RPC error with code
`INDEX_UNAVAILABLE` for every tool call except `get_index_status` while
its lifecycle state is `Indexing` or `Failed`. The error message MUST
include the current state (e.g. "indexing in progress" or "indexing
failed: <reason>") so the agent can decide whether to retry or surface
to its operator.

The `get_index_status` tool SHALL succeed in every state and SHALL
return a structured payload describing the lifecycle state.

#### Scenario: search_symbols during indexing

- **GIVEN** the MCP server is in the `Indexing` state
- **WHEN** the agent calls `tools/call search_symbols { ... }`
- **THEN** the response is a JSON-RPC error with code
  `INDEX_UNAVAILABLE`
- **AND** the error message contains the string `"indexing"`

#### Scenario: get_index_status during indexing

- **GIVEN** the MCP server is in the `Indexing` state with batch
  progress recorded
- **WHEN** the agent calls `tools/call get_index_status`
- **THEN** the response is success with `state: "indexing"`
- **AND** the response includes `progress` fields (files seen,
  symbols seen, current phase)
- **AND** the response is returned without delay (< 100ms)

#### Scenario: Tools become available after Ready

- **GIVEN** the MCP server has transitioned `Indexing → Ready`
- **WHEN** the agent calls any tool, e.g. `get_workspace_overview`
- **THEN** the tool returns its normal success payload, not
  `INDEX_UNAVAILABLE`

### Requirement: get_index_status returns lifecycle state

The `get_index_status` tool's response payload SHALL include a `state`
string field with one of `"indexing"`, `"ready"`, or `"failed"`.

When `state` is `"indexing"`, the payload SHALL include a `progress`
object with at least:
- `phase` (string) — current pipeline phase identifier
- `files_seen` (number)
- `symbols_seen` (number)

When `state` is `"failed"`, the payload SHALL include an `error`
string describing the failure.

When `state` is `"ready"`, the existing fields (`snapshot_id`,
`indexed_at`, `is_stale`, `reindex_in_progress`,
`fallback_from_parent_worktree`) SHALL all be populated as before.

#### Scenario: Status during indexing carries progress

- **GIVEN** the server is in `Indexing` and has processed two batches
- **WHEN** `get_index_status` is called
- **THEN** the response includes `state: "indexing"`
- **AND** `progress.phase` is a non-empty string
- **AND** `progress.files_seen` and `progress.symbols_seen` are
  non-negative numbers

#### Scenario: Status after failure carries error

- **GIVEN** the server is in `Failed` because the indexer subprocess
  exited with a non-zero status
- **WHEN** `get_index_status` is called
- **THEN** the response includes `state: "failed"`
- **AND** `error` is a non-empty string describing the failure

### Requirement: Progress notifications during indexing

While indexing is in progress, the MCP server SHALL emit rmcp
`notifications/message` log entries at info level summarizing pipeline
progress. Notifications SHALL be emitted at least:

- Once at the start of indexing.
- Once when the data ingest phase completes.
- Once at significant milestones (per implementation choice — typically
  per batch flush).
- Once when indexing finishes (success or failure).

Agents SHALL be able to observe indexing progress without polling
`get_index_status`, by listening for these notifications.

#### Scenario: Indexing emits start and end notifications

- **WHEN** the MCP server starts and triggers indexing
- **THEN** an info-level `notifications/message` is emitted with a
  payload signaling indexing started
- **AND** when indexing completes, a final info-level notification is
  emitted signaling completion
