## ADDED Requirements

### Requirement: Stylesheet ingest runs as a parallel ingest unit

During the ingest phase, the orchestrator SHALL run stylesheet ingestion as an
additional unit concurrent with the per-language code ingest units, streaming its
records through the same bounded channel to the DB writer.

#### Scenario: Stylesheet and code ingest concurrently

- **WHEN** a run includes both code and stylesheet roots
- **THEN** stylesheet ingestion proceeds concurrently with the code ingest units
  within the ingest phase

### Requirement: CSS-internal and class-usage resolution have distinct gates

The two post-producer resolution steps have different dependencies and SHALL be
gated independently:

- **CSS-internal resolution** (`@use`/`@import`/`@forward` → `imports`;
  `@extend`/`composes` → `extends_rule`) connects stylesheet nodes only, so it
  SHALL be gated **only on the stylesheet producer** completing — it MAY run
  concurrently with code ingest, NOT behind the code barrier.
- **Class-usage mining** (`uses_css_class`) attaches a code node as the source
  endpoint, so it SHALL be gated on **all code ingest units** completing (the
  existing post-code barrier), in addition to the stylesheet producer.

Stylesheet parsing and the class registry SHALL NOT be gated on either barrier —
they are the producer. Both resolution steps run before finalize/publish.

#### Scenario: CSS-internal resolves without waiting for code

- **WHEN** the stylesheet producer has finished but code ingest is still running
- **THEN** CSS-internal (`imports`/`extends_rule`) edges MAY already resolve
- **AND** `uses_css_class` edges are not yet emitted

#### Scenario: Usage edges resolve after the code barrier

- **WHEN** code ingest units are still running
- **THEN** `uses_css_class` edges are not yet emitted
- **AND** once all code ingest units complete, the usage step runs before publish

#### Scenario: A run with no code still publishes stylesheets

- **WHEN** a run includes stylesheet roots but no code
- **THEN** the stylesheet corpus (nodes + CSS-internal edges) is published
- **AND** the usage step resolves against an empty code graph without failing
