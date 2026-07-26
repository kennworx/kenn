## ADDED Requirements

### Requirement: Atlas axis read tools

The server SHALL expose the atlas's remaining axes as read tools, each answering
from the published snapshot:

- `list_domains` — cross-package domains; optional `domain` argument for one
  domain's spanned packages and central symbols.
- `list_contracts` — cross-package contracts; optional `contract` argument for
  one contract's implementers grouped by package.
- `list_documents` — first-party non-code directories.

Each tool SHALL be a read tool (no index build on the read path) and SHALL return
an empty list rather than an error when its axis is empty for the workspace.

#### Scenario: Domains tool answers from the snapshot

- **GIVEN** a Ready server over a snapshot whose analysis pass ran
- **WHEN** the agent calls `list_domains`
- **THEN** the earned-span domains are returned with their sizes and package spans
- **AND** no clustering is performed on the read path

#### Scenario: Contracts tool answers from the aggregate edges

- **GIVEN** a Ready server over a snapshot with `implements`/`extends_type` edges
- **WHEN** the agent calls `list_contracts`
- **THEN** each first-party interface whose implementers span more than one
  package is returned with its resolvable `pub_id` and package span

#### Scenario: An axis with no results is not an error

- **GIVEN** a workspace whose abstractions are all package-local
- **WHEN** the agent calls `list_contracts`
- **THEN** the response is an empty list and the call succeeds

### Requirement: list_packages reports the package concept's own metadata

`list_packages` SHALL report, for each package, the metadata the atlas package
concept carries and the query previously dropped: the package's root-module doc
(`description`, verbatim, absent when the package has none), its workspace-relative
manifest path (`resource`), its member-file count, and its per-directory file
counts. When a package is subdivided into component sub-areas, the response SHALL
name them.

`description` SHALL be copied verbatim and never synthesized.

#### Scenario: A documented package carries its doc

- **GIVEN** a package whose root module has a doc comment
- **WHEN** the agent calls `list_packages` for it
- **THEN** the response carries that doc verbatim as `description`

#### Scenario: An undocumented package omits the field

- **GIVEN** a package with no root-module doc
- **WHEN** the agent calls `list_packages` for it
- **THEN** no `description` is reported and none is invented
