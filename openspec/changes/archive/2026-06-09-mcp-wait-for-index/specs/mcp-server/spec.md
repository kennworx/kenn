## MODIFIED Requirements

### Requirement: Tools other than `get_index_status` fail fast while not Ready

The MCP server SHALL return a JSON-RPC error with code
`INDEX_UNAVAILABLE` for every tool call while its lifecycle state is
`Indexing` or `Failed`, **except for the status-class tools
`get_index_status` and `wait_for_index`**, which SHALL succeed in every
state. The error message MUST include the current state (e.g. "indexing
in progress" or "indexing failed: <reason>") so the agent can decide
whether to retry or surface to its operator.

The `get_index_status` tool SHALL succeed in every state and SHALL
return a structured payload describing the lifecycle state.
`wait_for_index` likewise SHALL succeed in every state — its purpose is
to be called precisely while the server is not yet Ready.

(The requirement name is retained for continuity; the normative set of
exempt tools is `get_index_status` and `wait_for_index`.)

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

#### Scenario: wait_for_index permitted during indexing

- **GIVEN** the MCP server is in the `Indexing` state
- **WHEN** the agent calls `tools/call wait_for_index { ... }`
- **THEN** the response is NOT an `INDEX_UNAVAILABLE` error
- **AND** the call blocks (up to its timeout) rather than failing fast

#### Scenario: Tools become available after Ready

- **GIVEN** the MCP server has transitioned `Indexing → Ready`
- **WHEN** the agent calls any tool, e.g. `get_workspace_overview`
- **THEN** the tool returns its normal success payload, not
  `INDEX_UNAVAILABLE`

## ADDED Requirements

### Requirement: `wait_for_index` blocks until the index settles

The MCP server SHALL expose a `wait_for_index` tool that blocks until
the index reaches a **settled** state or a caller-supplied timeout
elapses, whichever comes first. The index is *settled* when
`get_index_status` would report `state: "ready"` with
`reindex_in_progress: false`, or `state: "failed"`. It is *unsettled*
while `state: "indexing"`, or `state: "ready"` with
`reindex_in_progress: true`.

The tool SHALL accept an optional `timeout_ms` argument. When omitted it
SHALL default to a bounded value (30 000 ms), and the server SHALL clamp
any supplied value to a hard maximum (120 000 ms) so a tool call cannot
block indefinitely.

The response SHALL carry the same status payload `get_index_status`
returns, plus a boolean `timed_out` field: `false` when the tool
returned because the index settled, `true` when it returned because the
timeout elapsed while still unsettled. The tool SHALL NOT return
`INDEX_UNAVAILABLE` in any state.

While waiting, the tool SHALL NOT hold the lifecycle lock across its
wait intervals (it polls), so concurrent tool dispatch is never blocked.

#### Scenario: Returns promptly when already settled

- **GIVEN** the server is `Ready` with no reindex in progress
- **WHEN** the agent calls `wait_for_index { }`
- **THEN** the response returns without waiting
- **AND** `state` is `"ready"` and `timed_out` is `false`

#### Scenario: Blocks through indexing then returns ready

- **GIVEN** the server is `Indexing`
- **WHEN** the agent calls `wait_for_index { "timeout_ms": 60000 }`
- **AND** the pipeline completes and transitions to `Ready` before the
  timeout
- **THEN** the call returns after the transition with `state: "ready"`
  and `timed_out: false`

#### Scenario: Times out while still indexing

- **GIVEN** the server is `Indexing` and does not complete within the
  timeout
- **WHEN** the agent calls `wait_for_index { "timeout_ms": 1000 }`
- **THEN** the call returns after ~1000 ms with `timed_out: true`
- **AND** `state` reflects the still-unsettled state (e.g. `"indexing"`)

#### Scenario: Returns immediately on failed

- **GIVEN** the server is `Failed`
- **WHEN** the agent calls `wait_for_index { }`
- **THEN** the call returns without waiting with `state: "failed"` and
  `timed_out: false`

#### Scenario: Supplied timeout is clamped to the maximum

- **WHEN** the agent calls `wait_for_index { "timeout_ms": 10000000 }`
- **THEN** the effective wait SHALL NOT exceed the hard maximum
  (120 000 ms)
