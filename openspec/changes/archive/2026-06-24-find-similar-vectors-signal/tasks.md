## 1. Fix

- [x] 1.1 `find_similar_symbols` returns `Option<Vec<..>>` — `None` for a source
  with no committed vector — through the store chain (search/reader/handle). →
  verify: compiles; the no-vector branch returns `None`.
- [x] 1.2 Add `McpErrorCode::EmbeddingUnavailable` (`EMBEDDING_UNAVAILABLE`,
  `-32002`); the `find_similar` tool maps `None` to it with a `kenn embed`
  message; update the tool description. → verify: clippy clean.
- [x] 1.3 Regression test: a vectored symbol returns `Some` neighbours; a
  vector-less symbol returns `None`. → verify: passes.

## 2. Spec

- [x] 2.1 `mcp-symbol-search` delta: ADD a requirement that `find_similar` signals
  a missing committed vector distinctly from an empty result. → verify: `openspec
  validate`.

## 3. Gates

- [x] 3.1 `cargo clippy --workspace --all-targets` clean; `just crap-ci` green;
  `cargo fmt --all` last.
