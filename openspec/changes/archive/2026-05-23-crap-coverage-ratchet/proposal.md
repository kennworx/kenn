## Why

`cargo-crap` and a `just crap` recipe are wired up, but nothing is
committed and nothing is enforced — CRAP scores (cyclomatic complexity
weighted by test-coverage gaps) can regress silently on every commit. A
workspace scan already shows 40+ functions over the default threshold of
30, most carrying low or zero coverage. Fixing them all at once would
mean large, risky edits to long-stable code; doing nothing lets the
backlog grow unchecked.

## What Changes

- Add a committed CRAP **baseline** (`crap-baseline.json`) generated from
  a workspace-wide, coverage-instrumented run — the single reference the
  gate compares against.
- Add a `.cargo-crap.toml` pinning the threshold and analysis tolerances,
  and excluding `examples/`, `benches/`, and `tests/` — never
  coverage-instrumented, so their `—` coverage inflates the count
  meaninglessly.
- Add a `just crap-ci` recipe and a CI job that runs `cargo crap` against
  the baseline and fails on any regression *or* any new over-threshold
  function — no commit can make the codebase crappier.
- Perform a first **coverage-first** remediation pass: add tests for the
  pure test-gap offenders (over threshold because of missing coverage,
  not genuine complexity), then regenerate the baseline to lock the gains
  in.
- No production code behavior changes — remediation adds tests, it does
  not refactor working logic.

## Capabilities

### New Capabilities
- `crap-quality-gate`: the CRAP regression gate — a committed baseline, a
  CI check that fails on any regression or any new over-threshold
  function, exclusion of non-instrumented directories, and the
  coverage-first ratchet that lowers the baseline over time.

### Modified Capabilities
<!-- none — no existing capability's requirements change -->

## Impact

- `justfile` — a new `crap-ci` recipe; the existing `crap` recipe is
  unchanged.
- New committed files: `.cargo-crap.toml`, `crap-baseline.json`.
- CI configuration — a new job; needs the LLVM coverage tooling
  available.
- New tests across `kenn-store`, `kenn-indexer`, and the other crates —
  no `src/` logic changes.
