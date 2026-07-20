## 1. Consolidate producer registration

- [x] 1.1 Merge `build_driver` (`crates/kenn-cli/src/cmd_index.rs:105`) and
      `configure_runner` (`crates/kenn-indexer/src/workflow.rs:193`) into one
      function in `kenn-indexer`. Both the CLI (`kenn index`) and the
      workflow/MCP `index_workspace` path call it. Done: `configure_runner` is
      now the single `pub` fn (re-exported from `kenn_indexer`); it gained the
      missing `with_markdown` branch and the dropped dotnet `test_globs`.
- [x] 1.2 Remove the now-duplicate function (make the CLI use the shared one, or
      keep a thin re-export). No entry path constructs its own producer set.
      Done: `build_driver` deleted from `cmd_index.rs`; the CLI calls the shared
      `configure_runner`.

## 2. Verification

- [x] 2.1 Regression test: an MCP/`index_workspace` run with
      `[language.markdown] enabled = true` yields markdown nodes (fails today).
      Done: `crates/kenn-indexer/tests/markdown_producer_parity.rs` (a
      markdown-only repo indexes >0 nodes via `index_workspace`).
- [x] 2.2 The CLI and workflow/MCP paths register an identical producer set for a
      given config. Done: single source of truth (one `configure_runner`); the
      `configure_runner_handles_every_language_combo` table test exercises it.
- [x] 2.3 `cargo clippy --workspace --all-targets` clean; `just crap-ci` green;
      `cargo fmt --all` last. Done: clippy clean, CRAP passed, fmt applied.
