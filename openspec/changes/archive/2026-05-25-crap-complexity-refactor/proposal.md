## Why

`crap-coverage-ratchet` landed the gate (`.cargo-crap.toml`, `just crap-ci`,
`crap-baseline.json`) and remediated the easy 10 pure-function offenders.
33 functions still exceed CRAP threshold 30. They fall into two
categories that don't yield to "add a unit test" — they need different
treatment, captured here so the list survives the ratchet change's
archive.

The gate already prevents *new* over-threshold functions and any
regression of the baseline; this change pushes the baseline itself
down rather than holding it.

## What Changes

Three buckets of work, all reducing the size of `crap-baseline.json`:

### Bucket A — Lance reader scan functions (test-gap, fixture-heavy)

12 `GraphReader::scan_analysis_*` / `GraphReader::list_*` / similar
functions in `kenn-store/src/db/graph/reader.rs` and adjacent. They
require building real Lance datasets in tests to exercise the
record-batch scan loops. Effort is "write a `storage_fixtures.rs` helper
that materializes minimal analysis tables, then test each scan path
against it" — modest per test, large in aggregate.

### Bucket B — CLI command runners (refactor needed)

5 `run` / `run_async` functions in `kenn-cli/src/cmd_*.rs` orchestrate
argument parsing, store opening, pipeline invocation, and reporting in
one function — high CC because of branching on flags and error
handling. Refactor each into a small `parse_args → params → execute`
trio so the testable core is a pure function of `params`.

### Bucket C — Complexity offenders (true refactor needed)

- `render_into` @ `kenn-analyze/src/report.rs` (CC=41): the only function
  whose CC exceeds 30, so even 100% coverage leaves CRAP > 30. Split
  by section (per-table renderer methods).
- `run_async` @ `kenn-cli/src/cmd_index.rs` (CC=30): borderline; bucket-B
  refactor + a thin `cargo test --test cmd_index_smoke` should suffice.
- `embed_pending` @ `kenn-store/src/db/mod.rs` (CC=25 cov=76%): drove
  through bucket-A-style helper for the embed-coordination tests.

## Capabilities

### Modified Capabilities

- `crap-quality-gate`: gains the lower baseline; no behavior change.

## Impact

- `crap-baseline.json` regenerated and committed at the end (smaller list)
- `kenn-store/tests/storage_fixtures.rs`: new helper for building
  minimal analysis tables in tests (bucket A)
- `kenn-cli/src/cmd_*.rs`: shape change — parse/params/execute split
  (bucket B)
- `kenn-analyze/src/report.rs`: refactor `render_into` into smaller
  methods (bucket C)
- No new dependencies; no CI changes (CI gate already in place from
  `crap-coverage-ratchet`)

## Open Questions

- Is the parse/params/execute pattern (bucket B) the right shape, or
  should we use `clap`'s subcommand handlers more aggressively? Defer
  until first refactor reveals what's actually painful.
- Some `GraphReader::scan_*` functions are already exercised by
  end-to-end tests in `crates/kenn-mcp/tests/`. Coverage may credit
  them via the cross-crate run; need to verify before writing
  redundant unit tests.
