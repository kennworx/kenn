## ADDED Requirements

### Requirement: The query layer is transport-agnostic

Every read query the workspace answers SHALL live in a crate that depends on no
transport — symbol lookup, navigation, usages, search, the findings store, and
every atlas axis. That crate SHALL NOT depend on `rmcp` or on any other protocol
implementation, and the rule SHALL be enforced by the dependency graph rather
than by convention, so that violating it fails the build.

Queries SHALL be answered from an open snapshot rather than from server state. A
query SHALL NOT observe the daemon's lifecycle, its file watcher, or its
connected peer — none of which is a fact about the code being queried.

This is what makes a query testable without starting a server, and what lets the
CLI and the MCP server share one implementation instead of two that can drift.

#### Scenario: The query crate cannot reach a transport

- **WHEN** a transport dependency is added to the query crate
- **THEN** the workspace fails to build

#### Scenario: A query is answered without a server

- **GIVEN** an open snapshot reader and a configuration
- **WHEN** a query is invoked against them
- **THEN** it returns its result
- **AND** no server lifecycle was driven to a ready state

### Requirement: The lifecycle gate and the empty-snapshot gate are separate

A query SHALL refuse an empty snapshot with a structured error that distinguishes
a workspace with no enabled language from one that has not been indexed, rather
than returning a silent empty result. That refusal SHALL be a property of opening
a query context, because it is a fact about the snapshot and the configuration.

Refusing to serve because the index is not yet ready SHALL remain a property of
the host that owns the lifecycle, because it is a fact about a running process
and not about any snapshot.

When both conditions hold, the lifecycle refusal SHALL win. A caller that cannot
be served at all must not first be told something about a snapshot it was never
going to read.

#### Scenario: An empty snapshot is refused with a hint

- **GIVEN** an indexed workspace whose snapshot holds no symbols
- **WHEN** a query context is opened over it
- **THEN** the open fails with an empty-snapshot error carrying a configuration
  hint

#### Scenario: The lifecycle refusal outranks the empty-snapshot refusal

- **GIVEN** a host whose index is not yet ready
- **AND** a snapshot that holds no symbols
- **WHEN** a query is invoked
- **THEN** the error reports that the index is unavailable, not that the snapshot
  is empty

#### Scenario: The overview query tolerates an empty snapshot

- **GIVEN** a workspace whose snapshot holds no symbols
- **WHEN** the workspace overview is requested
- **THEN** it returns a result carrying the configuration hint rather than failing

### Requirement: Protocol error numbering belongs to the transport

Query errors SHALL carry a stable string code identifying the condition. Mapping
those conditions onto a protocol's numeric error space SHALL be done by the
transport that speaks that protocol, and SHALL NOT appear in the query layer.

The string code is a fact about what went wrong; the number is a convention of
one wire format, and a second front end renders the same error without it.

#### Scenario: A query error renders without a protocol

- **WHEN** a query error is surfaced by a non-protocol front end
- **THEN** its string code and message are rendered
- **AND** no protocol-specific numeric code is required to do so
