## MODIFIED Requirements

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
