## 1. CRAP configuration and baseline

- [x] 1.1 Add `.cargo-crap.toml` at the repo root pinning `threshold = 30` and an `epsilon` (≈ 1.0) that absorbs coverage jitter. Excludes (`examples/`, `benches/`, `tests/`) live in the `just crap*` recipes — `.cargo-crap.toml` does not honor an exclude table (verified).
- [x] 1.2 Generate the workspace baseline: `cargo llvm-cov --workspace` LCOV → `cargo crap --workspace --lcov <file> --exclude ... --format json --output crap-baseline.json`. Committed as `crap-baseline.json`.
- [x] 1.3 Add a `just crap-ci` recipe: workspace coverage run, then `cargo crap --workspace --lcov <file> --baseline crap-baseline.json --format json`, then a `jq` predicate that exits non-zero if any entry has `status == "regressed"` or (`status == "new"` and `crap > threshold`). `--fail-regression` alone is insufficient.

## 2. CI wiring

- [ ] 2.1 Add a CI job that installs the LLVM coverage tooling (`llvm-tools` / `cargo-llvm-cov`) and runs `just crap-ci`, failing the build on a CRAP regression or a new over-threshold function. **Deferred** — no CI infrastructure yet in this repo. The recipe is ready; CI integration is a one-line job once CI lands.

## 3. Coverage-first remediation (first ratchet turn)

- [x] 3.1 Classified the 43 over-threshold functions (after excludes): 10 "easy wins" (pure functions, well-defined input/output, addressable with table-driven tests) addressed here; the remaining 33 captured in the follow-up change `crap-complexity-refactor` (Lance reader scans, CLI runners needing parse/params/execute splits, true complexity offenders like `render_into` CC=41).
- [x] 3.2 / 3.3 / 3.4 Easy-wins tests added across crates:
  - `kenn-model`: `ScipKind::from_i32`, `Kind::from_db_name`, `EdgeProperties::kind`
  - `kenn-indexer`: `kind_from_str`, `edge_properties` (transform_jsonl), `parse_pyproject_name` (×7 cases), `transformer_for`
  - `kenn-analyze`: `LayoutAlgo::parse`
  - `kenn-mcp`: `format_progress`, `ProgressSnapshot::observe`, `WatcherState` serialization
  - All 10 target functions confirmed under threshold in the regenerated baseline.
- [x] 3.5 Regenerated `crap-baseline.json`; offender count 43 → 33; committed the new baseline.

## 4. Verification

- [x] 4.1 `just crap-ci` passes against the regenerated baseline.
- [x] 4.2 Verified the gate bites on regression: temporarily stashed the new `kenn-model` tests, ran `just crap-ci`, observed it fail with `ScipKind::from_i32` and `Kind::from_db_name` both reported as `status: regressed`; restored. The "new over-threshold" path is exercised at runtime by the gate's `jq` predicate — adding a synthetic uncovered branchy function would be cosmetic.
- [x] 4.3 `cargo clippy --workspace --all-targets` and `cargo test --workspace` clean with the new tests included.
