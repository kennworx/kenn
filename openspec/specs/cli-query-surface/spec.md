# cli-query-surface Specification

## Purpose
TBD - created by archiving change cli-tool-mirror. Update Purpose after archive.
## Requirements
### Requirement: CLI mirrors the MCP read + knowledge tool surface

The `kenn` CLI SHALL expose the read and knowledge tools as verb-grouped
subcommands, each a thin wrapper that resolves the workspace snapshot, invokes
the same `kenn_query::*` function the MCP server invokes, and renders the result.
The groups SHALL be: the top-level axis verbs `overview`, `packages`, `domains`,
`contracts`, `documents`, `tables`, and the grouped verbs `find`, `list`,
`check`, `findings`, `get`. The CLI SHALL NOT alter the MCP server's output, tool
wiring, or behavior.

Both front ends SHALL call one implementation. Mirroring is a claim that two
surfaces cannot disagree, and it holds only while there is a single function to
disagree about — a CLI that reimplemented a query would satisfy the letter of
this requirement and lose its point. Neither front end SHALL own the query
implementation, so that neither can quietly diverge from the other.

Every axis the atlas emits SHALL have a mirroring verb, so no axis is reachable
only by reading a generated file.

The following tools SHALL NOT be mirrored: `wait_for_index`, `watch_start`,
`watch_stop`, `debug_env`. `get_index_status` and `reindex` are already covered
by `kenn status` and `kenn index` and SHALL NOT be duplicated. These are the
tools that control a running daemon rather than answer a question about the
code, which is why the CLI has no use for them.

#### Scenario: a read tool is invoked from the CLI

- **GIVEN** an indexed workspace
- **WHEN** `kenn find symbol OrderHandler` runs
- **THEN** it returns the same result the `find_symbol` MCP tool returns for
  `{name: "OrderHandler"}`

#### Scenario: both front ends reach the same implementation

- **WHEN** the CLI verb and the MCP tool for one query are compared
- **THEN** each dispatches to the same query function
- **AND** neither crate holds a second implementation of it

#### Scenario: excluded tools have no command

- **WHEN** the CLI is invoked with `watch-start`, `wait-for-index`, or
  `debug-env`
- **THEN** no such subcommand exists

#### Scenario: every atlas axis has a verb

- **GIVEN** an indexed workspace whose atlas was built
- **WHEN** the CLI's subcommands are enumerated
- **THEN** a verb exists for each axis the atlas emits — packages, domains,
  contracts, documents, and tables

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

### Requirement: Axis verbs take an optional name for nested detail

Each axis verb SHALL accept an optional entity name or id. Bare, it SHALL list
every entity in that axis as flat scalar rows. With a name, it SHALL add that
entity's nested detail — a domain's spanned packages and central symbols, a
contract's implementers grouped by package, a package's typed coupling in both
directions.

A bare listing SHALL NOT include any entity's nested detail, because emitting it
for every entity is quadratic and unreadable at scale.

#### Scenario: bare listing is a flat table

- **WHEN** `kenn contracts` runs on a workspace with cross-package contracts
- **THEN** the default output is a header-once table
- **AND** every row carries the same scalar fields, the resolvable id first

#### Scenario: naming one entity adds its detail

- **WHEN** `kenn contracts <name>` runs
- **THEN** the response adds that contract's implementers grouped by package
- **AND** each implementer carries a resolvable id and its source location

#### Scenario: an empty axis prints an empty listing

- **GIVEN** a workspace whose abstractions are all package-local
- **WHEN** `kenn contracts` runs
- **THEN** an empty listing is printed and the command exits successfully

#### Scenario: an ambiguous name prints every match, not an error

- **GIVEN** two packages that each define a type with the same name
- **WHEN** `kenn contracts <that name>` runs
- **THEN** both contracts are printed, each tagged with its resolvable id
- **AND** the command exits successfully

### Requirement: The documents verb is its own subcommand group

`kenn documents` SHALL be its own verb rather than a flag on `kenn packages`,
wired so it can carry future subcommands and flags while a bare invocation lists
the axis. A `document` concept SHALL NOT be folded into the `packages` listing,
because mixing concept types would make that verb's rows non-uniform and drop its
default output out of table form.

#### Scenario: documents is a standalone verb

- **WHEN** the CLI's subcommands are enumerated
- **THEN** `documents` is a top-level verb
- **AND** `kenn packages` has no `--documents` flag

#### Scenario: packages rows stay uniform

- **WHEN** `kenn packages` runs on a workspace that also has document concepts
- **THEN** only package concepts are listed
- **AND** the output is a header-once table

