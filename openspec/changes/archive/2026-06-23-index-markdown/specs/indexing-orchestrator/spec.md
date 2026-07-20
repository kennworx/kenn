## ADDED Requirements

### Requirement: Markdown ingest runs as a parallel ingest unit

During the ingest phase, the orchestrator SHALL run markdown ingestion as an
additional unit concurrent with the per-language code ingest units, streaming
its records through the same bounded channel to the DB writer.

#### Scenario: Markdown and code ingest concurrently

- **WHEN** a run includes both code and markdown roots
- **THEN** markdown ingestion proceeds concurrently with the code ingest units
  within the ingest phase

### Requirement: Markdown-to-code resolution is gated on code-ingest completion

The orchestrator SHALL run markdown-to-code link resolution as a step that
begins only after all code ingest units have completed and before
finalize/publish. Markdown-to-markdown resolution SHALL NOT be gated on this
barrier.

#### Scenario: Code links resolve after the barrier

- **WHEN** code ingest units are still running
- **THEN** markdown-to-code edges are not yet resolved
- **AND** once all code ingest units complete, the resolution step runs before
  the snapshot is published

#### Scenario: A run with no code still publishes markdown

- **WHEN** a run indexes only markdown roots (no code units)
- **THEN** the markdown graph is resolved and published without waiting on a
  code barrier
