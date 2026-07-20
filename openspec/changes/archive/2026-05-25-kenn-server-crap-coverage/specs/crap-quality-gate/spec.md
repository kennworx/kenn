## ADDED Requirements

### Requirement: New over-threshold functions are remediated by added coverage when reachable

New functions that appear above the CRAP threshold SHALL be remediated by adding test coverage that drops them under the threshold whenever coverage is reachable in unit-test scope. A coverage drop (cube of `1 − coverage`) brings CC 6–8 functions under threshold at ~50% coverage, so the bar is low.

The baseline file is NOT a release valve for new code paths. Adding a new entry to `crap-baseline.json` SHALL be treated as a design decision that the function is genuinely uncoverable in unit scope, not as a convenient way to silence the gate.

#### Scenario: a new over-threshold function lands

- **WHEN** a change produces a new function above CRAP threshold 30
- **THEN** `just crap-ci` MUST report `status: new` for that function and exit non-zero
- **AND** the change MUST add test coverage (preferred) OR satisfy the grandfathering requirement below

### Requirement: Grandfathered entries are documented with rationale and a path back to coverage

When a function is genuinely uncoverable in unit-test scope (singleton initializers, process-spawn entrypoints, privileged OS calls, live ML model dependencies), it MAY be added to `crap-baseline.json` as a grandfathered entry. Each such entry SHALL be accompanied by a record in `openspec/.../crap-grandfather.md` (per-change) or the project-wide equivalent listing:

1. File and line of the function
2. CC and CRAP at the time of grandfathering
3. A specific reason for why coverage is not reachable in unit scope (e.g. "OnceLock global init shares state across tests in one process")
4. A path back to coverage — what change in test harness, dependency injection, or upstream API would make the function coverable

When the path-back condition becomes true, the baseline entry SHALL be removed in the same change that lands the coverage.

#### Scenario: a function is grandfathered with documentation

- **WHEN** a new over-threshold function is added to `crap-baseline.json` without test coverage
- **THEN** the same change MUST add a `crap-grandfather.md` entry recording file:line, CC, CRAP, the testability blocker, and the path back to coverage
- **AND** `openspec validate` for the containing change MUST pass

#### Scenario: cmd_server::status and cmd_server::start get coverage

- **WHEN** the `kenn-server-crap-coverage` change lands
- **THEN** `cmd_server::status` MUST report under CRAP 30 in the next `just crap-ci` run via the `render_status` extract + table test
- **AND** `cmd_server::start` MUST report under CRAP 30 via the `decide_start_mode` extract + table test
- **AND** neither function MUST appear in `crap-baseline.json` as a grandfathered entry

#### Scenario: llama_backend and spawn_daemon_and_wait are grandfathered with rationale

- **WHEN** the `kenn-server-crap-coverage` change lands
- **THEN** `kenn-embed::llama::llama_backend` and `kenn-cli::cmd_server::spawn_daemon_and_wait` MUST appear in `crap-baseline.json` as grandfathered entries
- **AND** `openspec/changes/kenn-server-crap-coverage/crap-grandfather.md` MUST contain a record for each, naming the singleton-init-state and child-process-spawn blockers respectively
