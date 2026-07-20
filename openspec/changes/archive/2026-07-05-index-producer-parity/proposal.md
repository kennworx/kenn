## Why

**Bug: MCP-triggered indexing silently skips markdown.** The index driver is
configured in two near-identical functions:

- `build_driver` (`crates/kenn-cli/src/cmd_index.rs:105`) — the CLI `kenn index`
  path. Registers csharp/rust/typescript/python/go/swift + **markdown** + css +
  html.
- `configure_runner` (`crates/kenn-indexer/src/workflow.rs:193`) — the
  workflow / MCP `index_workspace` path. Registers the same set **minus
  markdown** (has `with_css`/`with_html` at :237-242, but no `with_markdown`).

They are ~40 lines of the same `if config.language.X.enabled { runner.with_X() }`
copy, and they have drifted: the markdown branch (cmd_index.rs:150-152) was never
added to the workflow copy. Result: with `[language.markdown] enabled = true`, a
CLI index includes markdown nodes but an **MCP-triggered index does not** — the
agent's own indexing path is missing a whole content type, invisibly.

The root cause is the duplication, not markdown specifically: any producer added
to one function and not the other drifts the same way.

## What Changes

- **Consolidate** `build_driver` and `configure_runner` into a **single**
  producer-registration function (in `kenn-indexer`), called by both the CLI and
  the workflow/MCP entry paths. The CLI's `build_driver` becomes a thin
  re-export or is removed.
- This closes the markdown-on-MCP bug and makes producer registration a single
  source of truth, so no future producer can be present on one entry path and
  absent on another.

## Capabilities

### Modified Capabilities

- `indexing-orchestrator`: producer registration is a single source of truth,
  identical across the CLI and workflow/MCP index entry paths.

## Impact

- **Behavior:** MCP-triggered index runs now include markdown when it is enabled
  (today they do not). No other producer set changes — the CLI already had the
  full set; this brings the workflow/MCP path up to parity.
- **Scope:** a focused defect fix + de-duplication. Independent of, and a
  prerequisite for, `text-fallback-index` (whose new producer must register on
  both paths via this single function).
- **Test:** an `index_workspace`/MCP run over a repo with markdown enabled
  produces markdown nodes.
