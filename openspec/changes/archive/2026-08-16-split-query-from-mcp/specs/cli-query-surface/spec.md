## MODIFIED Requirements

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
