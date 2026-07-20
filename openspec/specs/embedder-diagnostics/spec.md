# embedder-diagnostics Specification

## Purpose
TBD - created by archiving change embedder-doctor. Update Purpose after archive.
## Requirements
### Requirement: An on-demand embedder self-check reports health or the exact cause

kenn SHALL provide a `doctor` command that actively probes the embedder by
embedding a trivial string through the process-global embedder and reports one of
three outcomes: **healthy** — the embedding dimension, measured latency, and the
active backend (in-process, remote daemon, or external URL); **disabled** — no
model configured, search is lexical-only; or **failed** — a one-line summary plus
the full underlying backend error text. The command's exit code SHALL distinguish
these outcomes. The probe SHALL exercise the actually-selected backend (not a
fresh isolated load), so it reflects the real runtime path including daemon
failures.

#### Scenario: healthy embedder

- **WHEN** `kenn doctor` runs with a working embedder
- **THEN** it prints the embedding dimension, latency, and the active backend
- **AND** exits with a success code

#### Scenario: no model configured

- **WHEN** `kenn doctor` runs with embedding disabled
- **THEN** it reports lexical-only / disabled rather than an error

#### Scenario: backend failure surfaces the raw cause

- **GIVEN** the selected backend fails to embed (e.g. the macOS fork+Metal bug)
- **WHEN** `kenn doctor` runs
- **THEN** it prints the one-line summary and the full underlying error text
- **AND** exits with a failure code

### Requirement: A real embedding failure is a distinct, visible state

A genuine embed-pass backend failure SHALL be reported as a `degraded` state
carrying the cause, distinct from both `ready` (embeddings present) and `disabled`
(no model configured). It SHALL NOT be collapsed into `ready`. This state SHALL be
observable through the index-status surfaces (`get_index_status`, `kenn status`).

#### Scenario: a swallowed backend error becomes observable

- **GIVEN** the embed pass hits a backend error while a model is configured
- **WHEN** index status is queried
- **THEN** the state is `degraded` with the error cause, not `ready`

#### Scenario: degraded is distinct from disabled

- **WHEN** embedding is off because no model is configured
- **THEN** the state is `disabled`, not `degraded` — the two are not conflated

