## Why

kenn's entire **query + knowledge surface** — symbol search, graph navigation,
findings, directives — is reachable only by speaking MCP over stdio. From a
terminal, a shell script, or a sub-agent that has a Bash tool but no MCP client,
there is no way to run `find_symbol` or `list_callers`. The only CLI commands
today are lifecycle (`init`, `index`, `status`, `rollback`, `embed`, `server`,
…).

The tool functions are already pure and public: every MCP tool is
`kenn_mcp::tools::X(state: &ServerState, args) -> Result<T: Serialize, McpError>`,
and the CLI already builds the same `ServerState` and tokio runtime in
`cmd_mcp`. Mirroring the read/knowledge tools as CLI subcommands is thin glue —
argument parsing plus one output renderer — over machinery that already exists.

Output should default to **TOON** (Token-Oriented Object Notation): the `find`
and `list` tools return `ListResponse<T> { items: Vec<T>, next }` — a uniform
array of rows, the exact case TOON collapses to a header-once table (~40–60%
fewer tokens, more skimmable). The remaining tools return single objects or
bespoke shapes; TOON still renders them, as nested key:value rather than a
table. `--json` opts out to the same JSON value the MCP server emits, for `jq`
and scripting.

## What Changes

- Add a **verb-grouped CLI query surface** mirroring the 29 read/knowledge MCP
  tools: `overview` (singleton), `find`, `list`, `check`, `findings`, `get`.
  Each subcommand is a thin wrapper that builds `ServerState`, calls the
  existing `kenn_mcp::tools::*` function, and renders the result.
- Add a shared **output renderer**: **TOON by default**, `--json` for the same
  JSON value the MCP server returns. A shared flag block mirrors the
  tool `Filters` (`--include-tests`, `--include-external`, `--kind`,
  `--language`, `--package`, `--file`) and pagination (`--page-size`,
  `--cursor`, `--all`).
- Naming hides internal jargon: a bare `kenn find <query>` is semantic search;
  the anchor-integrity sweep is `kenn check findings`; re-confirming a finding's
  anchor is `kenn findings touch`; storing is `kenn findings add`.

## Non-Goals

- **The MCP output format is not touched.** No change to `kenn-mcp`'s
  `Content::json` / envelope shape. (Emitting TOON from MCP is a separate,
  higher-stakes idea, explicitly out of scope here.)
- `wait_for_index`, `watch_start`, `watch_stop` are not mirrored — they are
  long-lived-process concerns; a one-shot CLI reads the live snapshot as-is.
- `debug_env` is not mirrored.
- `get_index_status` and `reindex` are already covered by `kenn status` and
  `kenn index`.

> **Scope note.** This change started CLI-only. Two follow-on directives pulled
> a small amount of `kenn-mcp` in: (1) a def-location read-path fix (a symbol
> with a spurious zero-range def caused a panic / wrong `#0`), and (2) making
> the tool `include_tests` default **universal** — `false` everywhere — so the
> CLI and MCP share one default (the graph-walk tools and `find_usages`
> previously defaulted / hard-coded `true`). Both are reflected in the modified
> capabilities below.

## Capabilities

### Added Capabilities

- `cli-query-surface`: the `kenn` CLI exposes the read + knowledge tool surface
  as verb-grouped subcommands with dual TOON/JSON output.

### Modified Capabilities

- `mcp-find-usages`: `find_usages` gains `include_tests` / `include_external`
  params and now **excludes** test and external references by default
  (was: tests always included).
- `mcp-symbol-search`: the search + graph-navigation tools share one universal
  `include_tests=false` default (graph-walk tools were `true`).

## Impact

- **Reach:** the query + knowledge layer becomes usable from shells, scripts,
  pipelines, and Bash-only agents — not just MCP hosts.
- **Cost:** one renderer + arg plumbing over already-public tool functions; no
  new query logic.
- **Wrinkles to accept:** the query-embedding tools (bare `find`, `find
  symbols`, `findings search|add|merge`, `findings directives --query`) pay
  embedder cold-start per one-shot invocation — mitigated by pre-warming that
  set before dispatch. A bare `find <single-word>` can collide with a
  subcommand name (see design D3).
