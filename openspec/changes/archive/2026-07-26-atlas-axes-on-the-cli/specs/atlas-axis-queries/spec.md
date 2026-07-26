## ADDED Requirements

### Requirement: Axis rules have exactly one implementation

Each domain/contract axis SELECTION rule SHALL have exactly one implementation,
shared by the atlas producer and the query surface — the membership floors, the
earned-span logic, the ranking, and the id dedupe, in `atlas/domains.rs` and
`atlas/contracts.rs`. Neither consumer SHALL define its own copy of a floor
(`MIN_DOMAIN_SIZE`, `MIN_PKG_MEMBERS`, `MIN_DOMAIN_LINKS`, `MIN_CONTRACT_PKGS`)
or a ranking rule.

Render caps (`MAX_CONTRACTS`, `MAX_CONTRACT_PKGS`, `MAX_IMPLEMENTERS_PER_PKG`)
are presentation policy, NOT selection, and SHALL remain atlas-side. They SHALL
NOT bound a query result.

The shared modules SHALL accept input-agnostic projections (anchor names, node
ids, weights, per-node metadata) rather than the indexer's `*Record` or the
store's `*Row` types, so each consumer projects its own inputs — the pattern
`atlas/coupling.rs` established.

#### Scenario: A threshold change affects both surfaces at once

- **GIVEN** the domain member floor is changed in the shared module
- **WHEN** the atlas is rebuilt and the query is re-run against that snapshot
- **THEN** both reflect the new floor
- **AND** neither surface contains a second definition of it

#### Scenario: Producer and query agree on the same snapshot

- **GIVEN** one snapshot with a published aggregate graph and analysis
- **WHEN** the atlas markdown and the query responses are both produced from it
- **THEN** the domain ids, titles, and sizes match
- **AND** the contract ids, kinds, and package spans match

#### Scenario: A render cap does not bound a query

- **GIVEN** a workspace with more qualifying contracts than the atlas renders
- **WHEN** the agent pages through the contracts axis
- **THEN** every qualifying contract is reachable
- **AND** the atlas's render cap has not silently truncated the result set

### Requirement: Axis queries are paginated

An axis listing SHALL accept pagination and SHALL be drainable to completion — no
axis result is bounded by a presentation cap. Where a response is nonetheless
bounded, it SHALL report the pre-cap total beside the rows it returned, so a
reader can see that something was withheld rather than mistake a truncation for
the whole set.

#### Scenario: A large axis is drainable

- **GIVEN** an axis with more entities than one page holds
- **WHEN** the caller drains every page
- **THEN** every entity is returned exactly once
- **AND** no cursor remains

#### Scenario: A bounded response names its total

- **WHEN** a response returns fewer rows than qualified
- **THEN** the pre-cap total is reported alongside them

### Requirement: The domains axis is queryable

The system SHALL answer a request for the workspace's cross-package domains from
a published snapshot, returning only the EARNED-span domains — the same subset
the atlas renders, never the raw clustering output. Each domain SHALL report its
id, its title (the hub symbol, ranked by intra-domain degree), its member count,
the number of packages it spans, and the count of cross-package links that
earned that span.

The query SHALL read the persisted analysis and the aggregate graph; it SHALL NOT
recluster on the read path.

#### Scenario: Bare listing returns earned domains only

- **WHEN** the agent requests the domains axis
- **THEN** every returned domain clears the member floor and the
  cross-package-link floor
- **AND** a raw community that spans packages only through a shared external type
  is absent

#### Scenario: A named domain carries its spanned packages

- **WHEN** the agent requests one domain by id
- **THEN** the response adds the packages it spans, each with its member count
  and its intra-domain link count
- **AND** the domain's most central member symbols, each with a resolvable id

#### Scenario: A repo with no cross-package clusters returns an empty list

- **GIVEN** a workspace whose communities are all within a single package
- **WHEN** the agent requests the domains axis
- **THEN** the response is an empty list, not an error

### Requirement: The contracts axis is queryable

The system SHALL answer a request for the workspace's cross-package contracts —
first-party interfaces, base classes, or protocols whose implementers span more
than one package — read directly from the `implements` / `extends_type` edges of
a published snapshot. Each contract SHALL report the contract type's own
resolvable `pub_id`, its name, its kind, the package it is defined in, its total
distinct implementer count, and the number of packages its implementers span.

Implementers and the contract itself SHALL be first-party and non-test. Counts
SHALL be the pre-cap totals so a capped response never reads as the whole truth.

#### Scenario: Cross-package only

- **GIVEN** an interface implemented only inside its own package
- **WHEN** the agent requests the contracts axis
- **THEN** that interface is absent — a single-package abstraction is the package
  concept's business

#### Scenario: A named contract carries its implementers per package

- **WHEN** the agent requests one contract
- **THEN** the response groups its implementers by package, widest first
- **AND** each implementer carries a resolvable `pub_id` and its source location

#### Scenario: An empty contracts axis is a valid answer

- **GIVEN** a workspace whose abstractions are all package-local
- **WHEN** the agent requests the contracts axis
- **THEN** the response is an empty list, not an error

### Requirement: The documents axis is queryable

The system SHALL answer a request for the workspace's first-party non-code
directories (the atlas's `document` concepts), each reporting its id, its title,
its path, and its member-file count. This axis SHALL NOT serve file contents.

#### Scenario: Listing tracked non-code directories

- **WHEN** the agent requests the documents axis
- **THEN** each first-party non-code directory the atlas tracks is listed with
  its file count
- **AND** no file contents are returned

### Requirement: An axis entity is addressed by query, not by id

An axis entity's name argument SHALL be treated as a query that matches either
the entity's display title or its resolvable `pub_id`. Titles are NOT unique —
two packages may each define a same-named type, which is why the atlas
disambiguates colliding contract slugs — so a title MUST NOT be the only way to
address an entity, and the collision-disambiguated form MUST NOT be required.

When a query resolves to more than one entity, the response SHALL return all
matches grouped by resolved target, each tagged with its `pub_id`. It SHALL NOT
return an error, and SHALL NOT require a second call to disambiguate.

#### Scenario: A pub_id resolves to exactly one entity

- **WHEN** the agent names a contract by its `pub_id`
- **THEN** exactly that contract is returned

#### Scenario: An ambiguous title returns every match in one response

- **GIVEN** two packages that each define a type with the same name
- **WHEN** the agent names that title
- **THEN** both contracts are returned, each tagged with its own `pub_id`
- **AND** the call succeeds rather than erroring on ambiguity

### Requirement: Axis listings are flat rows

Every axis listing SHALL return rows whose fields are all scalars, so the default
table output renders as a header-once table. Nested detail — a domain's spanned
packages, a contract's per-package implementers — SHALL be populated only when
the caller names a single entity.

The resolvable id SHALL be the first field of each row.

#### Scenario: A bare listing renders as a table

- **WHEN** the agent runs a bare axis listing
- **THEN** every row carries the same scalar fields
- **AND** the id is the first column

#### Scenario: Nested detail is opt-in by naming one entity

- **WHEN** the agent names a single domain or contract
- **THEN** the nested detail for that entity is included
- **AND** a bare listing of the same axis omits it
