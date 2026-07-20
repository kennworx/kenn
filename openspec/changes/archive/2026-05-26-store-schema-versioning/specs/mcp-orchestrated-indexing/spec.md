## ADDED Requirements

### Requirement: Schema-version mismatch routes through Failed for recovery

When the snapshot-open path returns a `SchemaMismatch` error (per `store-layout`'s `Snapshots carry a store-schema version` requirement), the MCP server SHALL map it to `LifecycleState::Failed` with an `error` string naming both the persisted snapshot version and the binary's expected `STORE_SCHEMA_VERSION`. The error message SHALL direct the reader to `SCHEMA_CHANGELOG.md` so the recipient can answer "what does v_N → v_M mean?" without code archaeology.

Because `LifecycleState::Failed` already has an established recovery path (`spawn_recovery_pipeline` on the next `reindex` tool call OR on the next staleness re-check), schema-mismatch SHALL converge to `Ready` through that machinery without a new lifecycle state. `get_index_status` reports the standard `failed` state with the schema-mismatch text in its `error` field — no new wire shape.

Schema-mismatch SHALL NOT cause the server process to exit. Reads return `INDEX_UNAVAILABLE` while the recovery reindex runs, matching the existing Failed-state behavior.

#### Scenario: A schema-mismatched snapshot is reported as failed, then recovered

- **GIVEN** the workspace's only retained snapshot persists `schema_version = 1`
- **AND** the running binary's `STORE_SCHEMA_VERSION` is `2`
- **WHEN** the MCP server starts
- **THEN** the lifecycle transitions to `Failed` with an `error` string naming both versions
- **AND** `get_index_status` returns `{state: "failed", error: "schema v1, binary expects v2; reindex required (see SCHEMA_CHANGELOG.md)"}`
- **AND** read tools return `INDEX_UNAVAILABLE`

#### Scenario: The reindex tool recovers a schema-stale state

- **GIVEN** the server is in `Failed` due to a schema-mismatch
- **WHEN** the agent calls the `reindex` tool
- **THEN** the server transitions to `Indexing` and runs the pipeline
- **AND** the new snapshot persists the current `STORE_SCHEMA_VERSION`
- **AND** the next snapshot open succeeds and the server reaches `Ready`
