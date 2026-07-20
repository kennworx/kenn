## 1. Output rendering (shared)

- [x] 1.1 Add `toon = "0.1"` and wrap it behind one internal helper (per design
      D2: depend, don't vendor). Verify its quoting/escaping on the free-form
      cases — a finding body / source span containing commas, colons, newlines,
      a leading `-`, and `"42"`/`"true"` — round-trips uncorrupted.
- [x] 1.2 Add `emit(value, Format)` in kenn-cli: `Format::Toon` (default) |
      `Format::Json` (pretty JSON — same *value* as MCP `Content::json`, not
      byte-identical: pretty vs compact).
- [x] 1.3 Flags: global `--include-tests` / `--include-external` (optional-value
      bool, bare = true, universal default false) + global `--json`; per-command
      `FilterArgs` (`--kind`/`--language`/`--package`/`--file`) + pagination
      (`--page-size`, `--cursor`, `--all`).

## 2. Command scaffolding

- [x] 2.1 Add the `find` / `list` / `check` / `findings` / `get` subcommand
      groups + top-level `overview` to the clap `Command` enum.
- [x] 2.2 One shared helper (`run_on_state`): build
      `ServerState::with_layout_config_and_model`, `bootstrap`, pre-warm the
      embedder when the subcommand embeds, `rt.block_on(producer)`, `emit`.
- [x] 2.3 Map `McpError` → CLI exit codes; render errors to stderr.

## 3. Leaf commands (thin: parse args → call `kenn_mcp::tools::*` → emit)

- [x] 3.1 `overview` → `get_workspace_overview`.
- [x] 3.2 `find` group: bare `<query>`→`semantic_search`; `symbol`→`find_symbol`,
      `symbols`→`search_symbols`, `at-location`→`find_at_location`,
      `similar`→`find_similar`, `usages`→`find_usages`.
- [x] 3.3 `list` group: callers, callees, implementers, overrides, usages,
      correspondences, in-scope, imports (`--direction` default `both`;
      `--import-kind` to avoid clashing with the filter `--kind`), module-files.
- [x] 3.4 `check` group: `links`→`check_links`, `css`→`check_css`,
      `findings`→`check_anchors`.
- [x] 3.5 `findings` group: `get`, `search`,
      `add <text>`→`store_finding` (repeatable `--parent`/`--tag`/`--anchor`),
      `merge <ids…>`→`merge_findings` (**required** `--text`, plus `--tag`),
      `directives <paths…>`→`find_directives` (`--query`),
      `predecessors`, `successors`,
      `touch <fnd_id>`→`record_anchor` (`--op attach|detach|rename`, default
      `attach`; `--anchor` for attach/detach, `--from`/`--to` for rename).
- [x] 3.6 `get` group: `symbol`→`get_symbol`, `source`→`get_source`.

## 4. Verification

- [x] 4.1 Each command prints TOON by default and valid JSON under `--json`; the
      `--json` value equals the tool's own result (same fn the MCP server calls).
- [x] 4.2 `--all` drains the cursor; `--page-size`/`--cursor` page manually.
- [x] 4.3 A `ListResponse` renders as a header-once TOON table; `next:` surfaces.
- [x] 4.4 `find <multi-word query>` runs semantic search; `--help` documents the
      single-word-collides-with-subcommand edge (design D3).
- [x] 4.5 Smoke tests in `cli_smoke.rs`: `command_tree_is_valid` (whole clap
      tree, catches arg collisions); `query_groups_render_on_empty_index`
      (`overview`/`findings get` in TOON + JSON); and
      `every_non_embedding_leaf_runs_without_panicking` (all 22 non-embedding
      leaves, which also covers the dispatch arms for the CRAP gate).
