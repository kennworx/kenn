## ADDED Requirements

### Requirement: The orchestrator registers one driver per enabled language

The orchestrator SHALL register the Swift JSONL indexer driver when
`[language.swift]` is enabled in configuration, as a sibling producer alongside the
C#, TypeScript, Rust, and Python drivers. The Swift driver SHALL reuse the existing
`JsonlIndexer` contract; when the Swift sidecar binary is absent the run SHALL
report the driver as unavailable (as for a missing C#/TS sidecar) rather than
failing the whole index.

#### Scenario: Swift driver registered when enabled

- **WHEN** configuration sets `[language.swift] enabled = true`
- **THEN** `configure_runner` registers a Swift JSONL driver in the runner

#### Scenario: Swift disabled by default

- **WHEN** no `[language.swift]` block enables it
- **THEN** no Swift driver is registered and Swift files are not indexed

#### Scenario: missing sidecar degrades gracefully

- **WHEN** Swift is enabled but the `kenn-swift` binary is not found
- **THEN** the run reports the Swift driver unavailable and other languages still
  index
