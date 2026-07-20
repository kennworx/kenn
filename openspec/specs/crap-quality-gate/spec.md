# crap-quality-gate Specification

## Purpose
TBD - created by archiving change crap-coverage-ratchet. Update Purpose after archive.
## Requirements
### Requirement: A committed CRAP baseline

The repository SHALL contain a committed CRAP baseline file produced by
`cargo crap` in JSON format. Coverage for the baseline SHALL be measured
from a workspace-wide test run, so a function exercised only by another
crate's tests is not recorded as uncovered. The baseline SHALL be
regenerated and re-committed whenever a remediation pass lowers function
scores, so the committed file always reflects the current best state.

The baseline SHALL be trimmed to over-threshold entries only (via the
`just crap-baseline` recipe). The full report has ~850 entries; the
trimmed baseline keeps only those above the threshold (~30). The gate
is semantically equivalent against either: an under-threshold function
that drifts over is reported as `status: new, crap > threshold` rather
than `status: regressed`, which the jq predicate handles identically.
Trimming keeps PR diffs reviewable as the baseline shrinks across
remediation passes.

#### Scenario: Baseline reflects workspace-wide coverage

- **WHEN** the baseline is generated
- **THEN** coverage is taken from a workspace-wide coverage-instrumented
  test run, not a per-crate one

#### Scenario: Baseline regenerated after remediation

- **WHEN** a remediation pass raises a function's coverage and lowers its
  CRAP score
- **THEN** the baseline file is regenerated from a fresh workspace run and
  committed, recording the lowered score

#### Scenario: Baseline contains only over-threshold entries

- **GIVEN** `just crap-baseline` is run after a remediation pass
- **WHEN** the resulting `crap-baseline.json` is inspected
- **THEN** every entry's `crap` field MUST be greater than the configured
  threshold
- **AND** functions with `crap` at or below the threshold MUST NOT appear
  in the baseline

#### Scenario: A drift-from-under-threshold function still fails the gate

- **GIVEN** a function NOT in `crap-baseline.json` (because its previous
  CRAP score was at or below threshold)
- **WHEN** its complexity rises or coverage falls so that its CRAP score
  exceeds the threshold
- **THEN** `just crap-ci` MUST fail with `status: new` and `crap >
  threshold` on that function

### Requirement: CI fails when the codebase gets crappier

CI SHALL run `cargo crap` against the committed baseline and SHALL exit
non-zero in either of two cases: an existing baselined function whose
CRAP score rises beyond the configured epsilon, or a function absent from
the baseline whose CRAP score exceeds the threshold. A score that is
unchanged, improved, or belongs to a new function at or under the
threshold SHALL NOT fail CI. The check SHALL NOT rely on `cargo crap
--fail-regression` alone, which does not fail on new functions.

#### Scenario: An existing function regresses

- **WHEN** a commit raises a baselined function's CRAP score beyond
  epsilon
- **THEN** the CI CRAP check exits non-zero and the commit is blocked

#### Scenario: A new crappy function

- **WHEN** a commit adds a function, absent from the baseline, whose CRAP
  score exceeds the threshold
- **THEN** the CI CRAP check exits non-zero and the commit is blocked

#### Scenario: A new function at or under the threshold

- **WHEN** a commit adds a function, absent from the baseline, whose CRAP
  score is at or under the threshold
- **THEN** the CI CRAP check passes

#### Scenario: Coverage improves an existing function

- **WHEN** a commit lowers a function's CRAP score relative to the
  baseline
- **THEN** the CI CRAP check passes

#### Scenario: Scores within epsilon of the baseline

- **WHEN** every function's CRAP score is within epsilon of its baseline
  value
- **THEN** the CI CRAP check passes, absorbing coverage-measurement
  jitter

### Requirement: Non-instrumented directories are excluded from CRAP analysis

CRAP analysis SHALL exclude the `examples/`, `benches/`, and `tests/`
directories of every crate, because their code is not
coverage-instrumented and would otherwise be scored as if uncovered.

#### Scenario: An example binary is not reported

- **WHEN** `cargo crap` analyzes the workspace
- **THEN** functions defined under `examples/`, `benches/`, or `tests/`
  appear in neither the report nor the baseline

### Requirement: CRAP remediation preserves runtime behavior

CRAP remediation SHALL preserve runtime behavior. A remediation pass
that lowers a function's CRAP score by adding tests SHALL NOT change
the function's signature, semantics, or observable outputs. A
remediation pass that lowers a function's CRAP score by refactoring
(splitting a high-CC function into smaller methods) SHALL produce a
result whose external behavior is unchanged — verified by the
function's own tests passing both before and after.

#### Scenario: Add-tests pass touches only test code and helpers

- **WHEN** a remediation pass targets a *test-gap* offender (CC ≤ threshold)
- **THEN** production code MUST NOT be modified except for visibility
  changes needed to test internals (e.g., `pub(crate)`)

#### Scenario: Refactor pass preserves the function's public contract

- **WHEN** a remediation pass targets a *complexity* offender (CC > threshold)
- **AND** the refactor splits the function or extracts helpers
- **THEN** the function's existing tests MUST pass without modification
- **AND** any new helpers introduced MUST be tested with the same
  scenarios that previously covered the original function's branches

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

