## 1. Fix

- [x] 1.1 In `compute_staleness_key` (`crates/kenn-store/src/staleness.rs`), map a
  dirty tracked file whose read fails (a deletion) to a deletion sentinel instead
  of dropping it; factor the per-file logic into `dirty_entry`. → verify: a
  deleted tracked file appears in the key.
- [x] 1.2 Regression tests: deleting a tracked file changes the key (sentinel
  entry present); `dirty_entry` hashes a present file and sentinels a missing
  one. → verify: tests pass with the fix, the key-change test fails without it.

## 2. Spec

- [x] 2.1 `workspace-staleness` delta: the git-form requirement states that a
  tracked deletion contributes a sentinel entry (not dropped), with a scenario.
  → verify: `openspec validate`.

## 3. Gates

- [x] 3.1 `cargo clippy --workspace --all-targets` clean; `just crap-ci` green;
  `cargo fmt --all` last.
