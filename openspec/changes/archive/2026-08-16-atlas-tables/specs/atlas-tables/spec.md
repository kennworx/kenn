## ADDED Requirements

### Requirement: The atlas emits a table concept per table in the graph

The atlas SHALL emit a `table` concept for every table node the graph contains, listing
every **reference site** that declares, modifies, or accesses it, each attributed to the
file and language it came from.

A reference site is whatever node carries the reference — a SQL statement in a `.sql`
file, an element in a markup document, or whatever a later producer contributes. The
concept SHALL name the site rather than only its file, and SHALL record which of
`DefinesTable`, `AltersTable`, or `AccessesTable` the site's edge carries.

The axis SHALL impose no earned-span floor. Other axes require an entity to span
packages before it earns a concept, because a narrower entity is local detail its
package concept already covers. A table belongs to no package and no other concept
covers it, so a floor would hide entities that have no other home in the map.

When the graph holds no tables the axis SHALL be empty rather than absent or failing,
so the atlas is correct on a workspace with no SQL producer enabled.

#### Scenario: A table reached from several files earns one concept

- **WHEN** one file declares a table, another modifies it, and a third queries it
- **THEN** one `table` concept is emitted for it
- **AND** all three reference sites are listed, each attributed to its file

#### Scenario: A table referenced once still earns a concept

- **WHEN** exactly one reference site in the workspace references a table
- **THEN** a `table` concept is emitted for it

#### Scenario: A workspace with no tables has an empty axis

- **WHEN** the atlas is built for a workspace whose graph holds no table nodes
- **THEN** the tables axis is present and empty
- **AND** the atlas build succeeds

### Requirement: The axis is a pure render-time projection

The tables axis SHALL be derived directly from the table nodes and their edges at
render time. It SHALL NOT depend on the clustering or analysis pass, SHALL NOT require
reindex invalidation, and SHALL NOT require a forced reindex to adopt.

This is what lets the axis widen without being changed: it lists whatever references
the graph holds, so a producer that later contributes table references from a markup or
source language is reflected automatically.

The projection SHALL read **per-site edges, not rolled-up ones**, and this axis
therefore differs from the axes that project aggregated nodes and edges. The difference
is deliberate and follows the question each axis answers. Package and contract axes
describe structure that exists at package granularity, so aggregation is their natural
input. A table's value is which site named it, and roll-up is defined to collapse a
document's members into the document — so projecting aggregated edges would report a
file where the graph knows an element, discarding the attribution this axis exists to
show.

#### Scenario: A concept names sites, not just their files

- **WHEN** one file contains two sites referencing the same table
- **THEN** the concept lists both sites
- **AND** they are distinguishable from one another, not merged into their file

#### Scenario: The axis is emitted without the analysis pass

- **WHEN** the atlas is built for a workspace with clustering disabled
- **THEN** the tables axis is still emitted with its concepts and references

#### Scenario: A new reference source widens a concept with no axis change

- **GIVEN** a table whose references all come from one language
- **WHEN** another producer contributes references to the same table
- **THEN** the concept lists the new references alongside the existing ones
- **AND** no change to the axis was required to include them

### Requirement: Concepts are ranked by reference breadth and marked by ownership

Concepts SHALL be ranked by how many distinct files reference the table, and then by
how many distinct languages those files span. A table that a migration, a markup
document, and application code all name is the one a reader is most likely looking for,
and breadth is the signal that says so.

Each concept SHALL record whether the table is **internal** — some reference site in the
workspace declares it — or **external**, meaning the workspace references a table it
does not declare. Both SHALL be listed; an external table is an ordinary finding, and
which tables a repository uses without owning is frequently the more useful answer.

Ownership SHALL be derived from the presence of a `DefinesTable` edge, and SHALL NOT be
read from the table node's `external` flag. That flag is set when the node is emitted,
which is before every producer has contributed: the `.sql` pass marks a table external
from the declarations *it* saw, so a table declared only by a markup document is written
to the snapshot as external and no later pass can correct it. Deriving from the edge is
independent of which producer ran first and stays correct as producers are added — the
same property this axis relies on to widen without being changed.

#### Scenario: A table declared outside SQL is marked internal

- **WHEN** no `.sql` statement declares a table
- **AND** a site in another language declares it
- **THEN** the concept marks the table internal
- **AND** the mark does not depend on which producer emitted the table node

#### Scenario: Breadth orders the listing

- **WHEN** one table is referenced from many files across several languages
- **AND** another is referenced from a single file
- **THEN** the first is ranked above the second

#### Scenario: Ownership is visible per table

- **WHEN** one table is declared in the workspace and another is only ever referenced
- **THEN** the first is marked internal and the second external
- **AND** both appear in the listing

### Requirement: Selection has exactly one implementation, shared with the query

Table selection SHALL have exactly one implementation — which tables appear, how they
are ranked, and how their references group — used by both the atlas producer and the
query surface. Neither consumer SHALL define its own copy of the ranking or grouping.

That implementation SHALL accept input-agnostic projections rather than the indexer's
record types or the store's row types, so each consumer projects its own inputs.

Render caps are presentation policy, SHALL remain in the producer, and SHALL NOT bound
a query result.

#### Scenario: Producer and query agree on one snapshot

- **GIVEN** one snapshot containing table nodes and their edges
- **WHEN** the atlas markdown and the query response are both produced from it
- **THEN** the table identities, ownership marks, and ordering match

#### Scenario: A render cap does not bound a query

- **GIVEN** a workspace with more tables than the atlas renders
- **WHEN** the tables are queried
- **THEN** every table is reachable through the query
- **AND** the atlas's own listing remains capped

### Requirement: The tables axis is queryable

The system SHALL answer a request for the workspace's tables. A bare listing SHALL
return flat rows whose fields are all scalars, ranked by breadth, each carrying the
table's resolvable id first, its name, its ownership mark, its distinct referencing-file
count, and its distinct language count.

Naming a single table SHALL return that table's references grouped by file, each
reference identifying its site and which edge kind it carries. Counts SHALL be
pre-cap totals, so a capped response never reads as the whole truth.

A name argument SHALL be treated as a query matching either the display name or the
resolvable id, and a query resolving to more than one table SHALL return all matches
tagged by id rather than an error.

#### Scenario: A bare listing renders as a table

- **WHEN** the tables axis is listed with no name
- **THEN** every row carries the same scalar fields
- **AND** the resolvable id is the first field

#### Scenario: Naming a table returns its references grouped by file

- **WHEN** a single table is named
- **THEN** its references are returned grouped by the file they came from
- **AND** each reference says whether it declares, modifies, or accesses the table

#### Scenario: An ambiguous name returns every match

- **WHEN** a name resolves to more than one table
- **THEN** all matches are returned, each tagged with its id
- **AND** no error is returned
