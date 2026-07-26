## MODIFIED Requirements

### Requirement: CLI mirrors the MCP read + knowledge tool surface

The `kenn` CLI SHALL expose the read and knowledge MCP tools as verb-grouped
subcommands, each a thin wrapper that resolves the workspace snapshot, invokes
the same `kenn_mcp::tools::*` function the MCP server calls, and renders the
result. The groups SHALL be: the top-level axis verbs `overview`, `packages`,
`domains`, `contracts`, `documents`, and the grouped verbs `find`, `list`,
`check`, `findings`, `get`. The CLI SHALL NOT alter the MCP server's output,
tool wiring, or behavior.

Every axis the atlas emits SHALL have a mirroring verb, so no axis is reachable
only by reading a generated file.

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

#### Scenario: every atlas axis has a verb

- **GIVEN** an indexed workspace whose atlas was built
- **WHEN** the CLI's subcommands are enumerated
- **THEN** a verb exists for each axis the atlas emits — packages, domains,
  contracts, and documents

## ADDED Requirements

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
