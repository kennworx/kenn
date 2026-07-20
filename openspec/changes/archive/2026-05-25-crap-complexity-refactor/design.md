# Design

The proposal split the work into three buckets (test-gap, refactor,
true-complexity) and budgeted nine rounds. Two design decisions
shaped what actually landed; one decision was deferred and survives
as accepted baseline debt.

## D1: Trim the committed baseline to over-threshold entries only

`crap-coverage-ratchet` committed the full `cargo crap` report
(~850 entries). Across nine remediation rounds the baseline diff
became the largest part of every PR. The full report is also
redundant at the gate layer — `just crap-ci` only cares about
entries above the threshold or those it tracks for regression.

The trim is semantically lossless. An under-threshold function
that later drifts over reports as `status: new, crap > threshold`
instead of `status: regressed`; the gate's jq predicate handles
both identically. Codified in `just crap-baseline` (the recipe
filters by `crap > threshold` before writing) and pinned by the
"Baseline contains only over-threshold entries" scenario in the
delta spec.

## D2: Two-shaped remediation passes — add-tests vs refactor

Every offender falls into one of two categories:

- **Test-gap** (CC ≤ threshold, low coverage): the function's
  cyclomatic complexity is acceptable; coverage is the limiting
  factor. Add tests, do not touch production code (except
  visibility tweaks, e.g. `pub(crate)`, where the existing
  internal API blocks an obvious test).
- **Complexity** (CC > threshold): even 100% coverage would leave
  CRAP > threshold. Refactor by extracting sub-functions; each
  helper inherits the orchestrator's existing tests through its
  call sites.

Codified in the "CRAP remediation preserves runtime behavior"
requirement of the delta spec: add-tests passes MUST NOT change
function signatures; refactor passes MUST keep existing tests
passing without modification.

The shape matters because it kept the nine rounds independently
reviewable — a pass either touched only tests or only refactored
one function. Reviewers didn't have to untangle mixed changes.

## D3 (deferred): ML embed path in unit-level fixtures

Four offenders cluster around the embedding subsystem:

- `DbReader::find_similar_symbols` (vector-with-results branch)
- `fetch_symbol_embedding`
- `embed_pending` (refactor target)

All require a working embed model in the test fixture to exercise
their non-early-return branches. The round-7 commit declared this
out of scope for unit-level fixtures — the embed-model load path
is too heavy for a fast test loop and the model itself is the
subject of a separate future change (`embedding-model-update`,
memory `[[project_model_update_deferred]]`).

These four entries remain in the committed baseline as accepted
debt. They unblock when `embedding-model-update` lands a testable
embed path, at which point a follow-up CRAP change can pick them
up. The gate keeps them from regressing further in the meantime.

## Outcome

`crap-baseline.json` shrank from 43 offenders to 4. The 4 remaining
are the D3-deferred items plus `resolve_roots_and_maybe_rebind`
introduced by `mcp-roots-discovery` (will be remediated in that
change). 91% of the original baseline was retired across nine
rounds.

The gate was verified to bite in a live regression demonstration
(task 5.3): mutating one baseline entry's `crap` field downward
caused `just crap-ci` to fail with `status: regressed`; restoring
the baseline returned the gate to pass.
