# cli-query-surface Specification

## Purpose
TBD - created by archiving change cli-tool-mirror. Update Purpose after archive.
## Requirements
### Requirement: CLI mirrors the MCP read + knowledge tool surface

The `kenn` CLI SHALL expose the read and knowledge MCP tools as verb-grouped
subcommands, each a thin wrapper that resolves the workspace snapshot, invokes
the same `kenn_mcp::tools::*` function the MCP server calls, and renders the
result. The groups SHALL be: a top-level `overview`, and `find`, `list`,
`check`, `findings`, `get`. The CLI SHALL NOT alter the MCP server's output,
tool wiring, or behavior.

The following tools SHALL NOT be mirrored: `wait_for_index`, `watch_start`,
`watch_stop`, `debug_env`. `get_index_status` and `reindex` are already covered
by `kenn status` and `kenn index` and SHALL NOT be duplicated.

#### Scenario: a read tool is invoked from the CLI

- **GIVEN** an indexed workspace
- **WHEN** `kenn find symbol OrderHandler` runs
- **THEN** it returns the same result the `find_symbol` MCP tool returns for
  `{name: "OrderHandler"}`

#### Scenario: excluded tools have no command

- **WHEN** the CLI is invoked with `watch-start`, `wait-for-index`, or
  `debug-env`
- **THEN** no such subcommand exists

### Requirement: Dual TOON / JSON output

Every query subcommand SHALL render its result as TOON by default and as JSON
when `--json` is passed. The `--json` payload SHALL be the serialization of the
same value the corresponding MCP tool returns. TOON output SHALL encode a
uniform array of objects as a single header row followed by one row per item.

#### Scenario: default output is TOON

- **WHEN** a query command returns a `ListResponse` of uniform rows
- **THEN** the field names are printed once as a header and each item is one row
- **AND** the `next` cursor value is present in the output

#### Scenario: --json returns MCP-parity payload

- **WHEN** the same command runs with `--json`
- **THEN** the output is JSON equal to the value the corresponding MCP tool
  returns for the same arguments

### Requirement: Universal test/external flags and shared filter/pagination flags

`--include-tests` and `--include-external` SHALL be global (available on every
command). Each SHALL be an optional-value boolean: given without a value it
SHALL mean `true`; given `=true`/`=false` it SHALL take that value; absent it
SHALL take a single universal default of `false`. The CLI SHALL send the
resolved value explicitly to every tool that accepts it, so a tool's own
default does not apply. Subcommands whose tool accepts `Filters` SHALL also
expose `--kind`, `--language`, `--package`, and `--file`. Paginating
subcommands SHALL expose `--page-size` and `--cursor`, plus `--all` which
drains the cursor and returns every page.

#### Scenario: --all drains pagination and preserves trailing metadata

- **GIVEN** a result that spans multiple pages
- **WHEN** the command runs with `--all`
- **THEN** all items across all pages are returned, no `next` cursor remains,
  and any non-`items`/`next` fields from the final page are preserved

#### Scenario: include flags are universal and tri-state

- **WHEN** any query command runs with no include flags
- **THEN** the tool is called with `include_tests = false` and
  `include_external = false`
- **AND WHEN** it runs with `--include-tests` (no value)
- **THEN** the tool is called with `include_tests = true`
- **AND WHEN** it runs with `--include-tests=false`
- **THEN** the tool is called with `include_tests = false`

### Requirement: Surface naming hides internal jargon

The CLI SHALL name commands for their user intent, not their internal
mechanism. A bare `kenn find <query>` (no subcommand) SHALL run semantic search.
The finding-anchor integrity sweep SHALL be `kenn check findings`. Re-confirming
or changing a finding's anchor SHALL be `kenn findings touch` with
`--op attach|detach|rename` (default `attach`). Storing a finding SHALL be
`kenn findings add`.

#### Scenario: bare find runs semantic search

- **WHEN** `kenn find order cancellation flow` runs
- **THEN** it invokes `semantic_search` with that query

#### Scenario: anchor operations are named without "anchor"

- **WHEN** `kenn check findings` runs
- **THEN** it invokes `check_anchors`
- **AND WHEN** `kenn findings touch <fnd_id>` runs with no `--op`
- **THEN** it invokes `record_anchor` with op `attach`

