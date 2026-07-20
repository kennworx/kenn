## ADDED Requirements

### Requirement: A committed CRAP baseline

The repository SHALL contain a committed CRAP baseline file produced by
`cargo crap` in JSON format. Coverage for the baseline SHALL be measured
from a workspace-wide test run, so a function exercised only by another
crate's tests is not recorded as uncovered. The baseline SHALL be
regenerated and re-committed whenever a remediation pass lowers function
scores, so the committed file always reflects the current best state.

#### Scenario: Baseline reflects workspace-wide coverage

- **WHEN** the baseline is generated
- **THEN** coverage is taken from a workspace-wide coverage-instrumented
  test run, not a per-crate one

#### Scenario: Baseline regenerated after remediation

- **WHEN** a remediation pass raises a function's coverage and lowers its
  CRAP score
- **THEN** the baseline file is regenerated from a fresh workspace run and
  committed, recording the lowered score

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

When the CRAP gate's remediation lowers a function's score, it SHALL do
so by adding tests that raise the function's coverage, leaving its
runtime behavior unchanged. A function whose CRAP score stays over the
threshold even at high coverage — because cyclomatic complexity is the
dominant term — SHALL be addressed as a separate refactor concern,
captured in a follow-up change proposal with its own correctness
guarantees, rather than refactored as part of CRAP remediation.

#### Scenario: A test-gap offender

- **WHEN** a function exceeds the CRAP threshold and would fall under it
  once its coverage reaches a realistic level
- **THEN** it is addressed by adding tests covering it, and its recorded
  behavior is unchanged

#### Scenario: A genuinely complex offender

- **WHEN** a function exceeds the threshold mainly because of high
  cyclomatic complexity
- **THEN** it is captured in a follow-up change proposal as a refactor
  concern, not addressed as CRAP remediation
