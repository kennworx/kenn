## ADDED Requirements

### Requirement: Snapshots carry a store-schema version that readers strictly check

`kenn-store` SHALL expose a single `STORE_SCHEMA_VERSION: u32` constant. Every published snapshot SHALL persist this value in its existing metadata blob (alongside `indexed_at`, snapshot id, and other run-level fields — no new file, no new I/O surface).

On every snapshot-open path — cold start, hot-reload after a background reindex publish, and fallback-to-parent-worktree resolution — the reader SHALL compare the persisted value to its own compiled-in `STORE_SCHEMA_VERSION` and SHALL refuse to serve the snapshot when they are not strictly equal. A snapshot whose `meta.json` exists but lacks the `schema_version` field SHALL be treated as version `1` (the pre-versioning convention). A snapshot directory with no `meta.json` at all SHALL bypass this check — the lifecycle's publish protocol refuses to mark such a run as published anyway, so the absent-meta branch never reaches a real reader on a properly-published snapshot, and the bypass keeps raw-`open_writer` test fixtures working.

A schema mismatch SHALL surface as a typed `SchemaMismatch` error from the store-open API, distinct from "snapshot not found" or "snapshot corrupt". Consumers (the MCP lifecycle, the CLI) map this error to their own user-facing form per the `mcp-orchestrated-indexing` and CLI conventions; the store itself MUST NOT mask the mismatch as a generic open failure.

Bumping `STORE_SCHEMA_VERSION` SHALL be paired with a new entry in `crates/kenn-store/SCHEMA_CHANGELOG.md` describing what changed and why prior snapshots cannot be read. The changelog is shipped with the source tree (not written into snapshots) and exists to give a future debugger a single place to answer "what does v_N mean?". Discipline around the pairing is enforced by code review, not tooling.

#### Scenario: A snapshot written by an older binary is rejected on open

- **WHEN** a snapshot's persisted `schema_version` is `1` and the current binary's `STORE_SCHEMA_VERSION` is `2`
- **THEN** the store-open API MUST return a `SchemaMismatch` error naming both versions
- **AND** no rows from that snapshot MAY be served to any reader

#### Scenario: A snapshot written by the current binary opens normally

- **WHEN** a snapshot's persisted `schema_version` equals the current `STORE_SCHEMA_VERSION`
- **THEN** the store-open API MUST succeed
- **AND** reads MUST proceed as today

#### Scenario: A pre-versioning snapshot's meta lacks the field and is treated as version 1

- **WHEN** a snapshot's `meta.json` exists and parses but has no `schema_version` field
- **THEN** the reader MUST treat it as version `1`
- **AND** the strict-equality check MUST apply (binaries with `STORE_SCHEMA_VERSION >= 2` reject it)

#### Scenario: A snapshot directory with no meta.json bypasses the version check

- **WHEN** a snapshot directory has no `meta.json` at all (a raw `open_writer` fixture, an in-progress unpublished run, or test scaffolding that skipped the indexer's meta stamp)
- **THEN** the schema-version check MUST be bypassed — the open MUST proceed
- **AND** this bypass MUST NOT widen to any other meta-file-presence case (a present-but-malformed `meta.json`, for example, retains the v1 default)
