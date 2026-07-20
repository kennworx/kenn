## 1. Fix

- [x] 1.1 `code_node_resolver` (`crates/kenn-store/src/db/sqlite/reader/fetch.rs`)
  selects `pub_id` directly instead of `format!("{language}:{pub_id}")`. → verify:
  the resolver set matches the canonical id `find_symbol` returns.
- [x] 1.2 Regression test: build the resolver from a seeded symbols table; the
  canonical pub_id resolves, the `{language}:` doubled form does not, an absent
  symbol is not contained. → verify: passes with the fix, fails without it.

## 2. Spec

- [x] 2.1 `findings-store` delta: the staleness requirement names the canonical
  code-node id as the resolution key, with a scenario. → verify: `openspec
  validate`.

## 3. Gates

- [x] 3.1 `cargo clippy --workspace --all-targets` clean; `just crap-ci` green;
  `cargo fmt --all` last.
