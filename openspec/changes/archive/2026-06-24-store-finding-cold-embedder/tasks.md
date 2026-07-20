## 1. Fix

- [x] 1.1 In `store_finding` (`crates/kenn-mcp/src/tools/findings.rs`), map an
  `EmbedderStarting` error from the pre-embed to `text_vec = None` instead of
  propagating it, mirroring `find_directives`. → verify: clippy clean; the write
  path no longer depends on a warm embedder.

## 2. Spec

- [x] 2.1 `findings-mcp` delta: the `store_finding` requirement gains the
  cold-embedder degrade behavior + a scenario. → verify: `openspec validate`.

## 3. Gates

- [x] 3.1 `cargo clippy -p kenn-mcp` clean; `just crap-ci` green; `cargo fmt
  --all` last.
