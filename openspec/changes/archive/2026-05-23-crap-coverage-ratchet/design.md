## Context

`cargo-crap` computes `CRAP(f) = CC(f)² · (1 − coverage(f))³ + CC(f)` — a
function is "crappy" when it is both complex *and* under-tested. The
`just crap` recipe generates an LCOV file with `cargo llvm-cov` and feeds
it to `cargo crap`. A first run reports ~17 over-threshold functions in
`kenn-store` and ~20 in `kenn-indexer`, with the other crates unmeasured.
The dominant term for nearly all of them is `(1 − coverage)³`: they are
ordinary code at 0–35 % coverage, not pathologically complex.

A per-crate coverage run (`cargo llvm-cov --package X`) executes only
crate `X`'s own tests, so a function exercised by a downstream crate's
integration tests reads as 0 % — a measurement artifact, not a real gap.

## Goals / Non-Goals

**Goals:**
- A committed baseline and a CI gate, so no commit can raise any
  baselined function's CRAP score or land a new function above the
  threshold.
- A first coverage-first remediation pass that genuinely lowers the
  baseline.
- A repeatable "ratchet" loop: raise coverage → regenerate baseline →
  the gate holds the new line.

**Non-Goals:**
- Refactoring stable, high-complexity functions to cut cyclomatic
  complexity — separate, higher-risk work.
- Driving every function under threshold in this change; the ratchet does
  that incrementally.
- Changing the threshold (stays at the default 30) or introducing
  per-crate baselines.
- SARIF / GitHub Code Scanning output — deferred to the GitHub migration
  (Decision 7).

## Decisions

### 1. The baseline comes from a `--workspace` run, never per-crate

Per-crate coverage understates reality (tests live downstream). The
baseline and the gate both run `cargo llvm-cov --workspace`, so a
function's coverage reflects every test that touches it. Cost: the
workspace run includes the slow embedding-model tests — accepted, the
baseline is generated rarely and CI runs it as one job.

### 2. The gate is a baseline-delta check, not `--fail-above` or `--fail-regression` alone

`--fail-above` fails when *any* function exceeds the threshold —
unshippable today against 40+ pre-existing offenders. The gate instead
compares every function to `crap-baseline.json` and fails on two
conditions:

- an **existing** (baselined) function whose CRAP score rose beyond
  `epsilon` — cargo-crap's `status: "regressed"`;
- a **new** function, absent from the baseline, whose CRAP score exceeds
  the threshold — `status: "new"` with `crap > threshold`.

Verified during review: `cargo crap --fail-regression` covers only the
first — it exits 0 even when a brand-new CRAP-300 function appears (the
tool categorizes it `new`, not `regressed`). Relying on
`--fail-regression` alone would let new untested, complex code straight
through, defeating the gate. So `just crap-ci` runs `cargo crap --baseline
crap-baseline.json --format json` and applies a `jq` predicate over the
per-entry `status` field for both conditions. The baseline holds the line
for known code; the threshold holds it for new code; neither can slip. A
new function *under* threshold passes — new code is allowed, new *crappy*
code is not.

The predicate does not need a `status: "moved"` term. Verified
empirically: cargo-crap matches functions by name (`line` is a display
field only). An in-file move whose score is unchanged classifies as
`unchanged`; an in-file move that also worsens the score classifies as
`regressed`. A cross-file move with an unchanged score classifies as
`moved` (with `previous_file` recording the old location); a cross-file
move whose score also changed overrides to `regressed` or `improved`.
So `"moved"` always means an unchanged score across a file move — by
definition not a CRAP issue — and every score increase falls into
`regressed` regardless of whether the function also moved.

### 3. `crap-baseline.json` is committed and regenerated deliberately

The baseline lives in the repo as the single source of truth.
Regenerating it is an explicit, reviewed commit — after a remediation
pass (to lock in gains), or when a legitimate change moves scores. A
drive-by score increase surfaces as a CI failure, not a silent baseline
edit.

### 4. Coverage-first remediation

CRAP is dominated by `(1 − coverage)³`. Raising a 0 %-coverage function
to 80 % collapses its score far more than shaving a branch would, and
adds tests rather than editing working logic — lower risk, and consistent
with the repo's "don't refactor what isn't broken" rule. The first pass
targets only **test-gap offenders**: those that drop under threshold once
coverage reaches a realistic level (~80 %). Functions that stay over
threshold even well-covered — because cyclomatic complexity is the
dominant term — are recorded as deferred refactor work, not touched here.

### 5. `examples/`, `benches/`, `tests/` are excluded

They are never coverage-instrumented, so they report `—` coverage and
score as if 0 %-covered — pure noise in the count. Excluded via
`.cargo-crap.toml` if the tool honors exclusions there, otherwise via
`--exclude` flags in the `just` recipes (verified at apply time).

### 6. An epsilon absorbs coverage jitter

Coverage instrumentation is not perfectly deterministic — a function's
line coverage can shift slightly between runs, nudging its CRAP score.
cargo-crap's `status` classification treats deltas within `epsilon` as
`unchanged`, so the gate's `status == "regressed"` check (Decision 2) is
already epsilon-filtered. A small epsilon (≈ 1.0, tunable) prevents
flaky CI failures from sub-line jitter without masking real regressions.

### 7. SARIF output is deferred to the GitHub migration

`cargo crap --format sarif` emits SARIF 2.1.0, which GitHub Code Scanning
ingests to annotate pull requests inline. This repository is not yet on
GitHub, so SARIF has no consumer today — the baseline-delta `jq`
predicate (Decision 2) is the whole gate. When the repo migrates to GitHub, a follow-up
change adds a `--format sarif` run and uploads the result via
`github/codeql-action/upload-sarif`, layering inline PR annotations on
top of the pass/fail gate. Recorded here so the migration checklist
picks it up; not built now, because with no consumer it would be dead
output. SARIF is incompatible with `--baseline`, so it is a *second*
`cargo crap` invocation, not a flag on the gating run.

## Risks / Trade-offs

- **Coverage nondeterminism → flaky gate** → Decision 6's epsilon; widen
  it if flakes persist.
- **The workspace coverage run is slow** (embedding-model tests) → one CI
  job; cache the toolchain. Acceptable for a per-PR gate.
- **LLVM tooling in CI** → the gate needs `llvm-cov` / `llvm-profdata`;
  CI must install the `llvm-tools` component. The `just` recipe already
  handles a Homebrew-rustc host locally.
- **Baseline built on a different platform than the gate runs on** →
  coverage instrumentation differs across OS / LLVM versions, so a
  baseline generated locally can show systematic (not jitter) score
  shifts when the gate runs in CI → false `regressed` verdicts on the
  first run → generate `crap-baseline.json` in the same environment the
  gate runs in (the CI runner), and regenerate it there.
- **A legitimate complexity increase is blocked** → intended; the author
  offsets it with coverage or regenerates the baseline in a reviewed
  commit.

## Open Questions

- None outstanding. SARIF / GitHub Code Scanning is resolved as a
  deferral — see Decision 7.
