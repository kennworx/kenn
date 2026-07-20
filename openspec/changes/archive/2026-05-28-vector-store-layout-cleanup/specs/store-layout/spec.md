## ADDED Requirements

### Requirement: Deferred runs-centric placements have direct test coverage

The deferred runs-centric layout claims SHALL hold true in code and SHALL be backed by direct tests: the indexer SHALL be exercised against per-language JSONL files written at `<derived_root>/runs/{id}/{lang}.jsonl` (not at multi-file driver-specific names); the findings store SHALL round-trip writes and reads against a Lance dataset at `<derived_root>/runs/{id}/lance/findings/` across an indexer pass; the deprecated path accessors `findings_local_dir()` and `embed_lock_path()` SHALL be removed from `Layout`; the `embed-locks/` directory SHALL no longer be created by any code path. If a workspace's `<derived_root>` already contains a stale `embed-locks/` directory from a prior layout, the indexer MAY sweep it at startup; this MUST NOT block normal operation.

#### Scenario: indexer reads JSONL from the runs-centric path

- **WHEN** an indexer pass runs against a workspace with a
  language driver that emits JSONL frames
- **THEN** the driver writes its frames to
  `<derived_root>/runs/{id}/{lang}.jsonl` (one file per language)
- **AND** the indexer ingests those frames from that path
- **AND** no `kenn-dotnet-stream-*.jsonl` or other multi-file
  driver-specific name lives outside `runs/{id}/`

#### Scenario: findings round-trip across an indexer pass via the runs-local mirror

- **GIVEN** a workspace with findings written via `kenn find`
- **WHEN** an indexer pass runs to completion and `live` is repointed
- **THEN** the new run's `lance/findings/` Lance dataset contains
  the findings
- **AND** subsequent reads through the public findings-store API
  return the same records
- **AND** no `<derived_root>/findings/` directory remains as the
  primary read path

#### Scenario: deprecated accessors and embed-locks directory are absent

- **WHEN** the workspace's `Layout` is resolved
- **THEN** the `Layout` type exposes no `findings_local_dir()` or
  `embed_lock_path()` accessor
- **AND** the indexer creates no `<derived_root>/embed-locks/`
  directory at any point during a normal pass
