## Why

Once tables are in the graph, they are the one entity a repository's schema, its
migrations, its mapper files, and its application code all name in common. Nothing in
the atlas surfaces that. The axes today are packages, cross-package domains,
cross-package contracts, components, and document directories — all organised around
code structure, none of which a table belongs to. A table is not in a package, so no
existing concept covers it, and a reader who wants "what touches `orders`, and where"
has to compose graph queries by hand.

That is the same gap the contracts axis filled for interfaces: an entity whose value is
precisely that it spans things, invisible in a map organised by containment.

Tables are also unusually well suited to being a map. A real repository measured during
design carried 128 distinct tables against tens of thousands of code symbols — small
enough to enumerate honestly, unlike a symbol axis.

## What Changes

- Add a **tables** axis to the atlas: one `table` concept per table in the graph,
  listing every statement that declares, modifies, or accesses it, grouped by the file
  and language the reference came from.
- Derive the axis as a **pure projection** of the table nodes and their edges at render
  time, following the contracts axis: no clustering, no analysis pass, no reindex
  invalidation, and no `--force` to adopt. This is what makes the axis grow on its own
  — it renders `.sql` references today and picks up XML and code references as those
  producers land, with no change to the axis itself.
- Rank concepts by **reference breadth**: how many distinct files reference the table,
  then how many distinct languages. A table named by a migration, a mapper document,
  and application code is the architecturally interesting one, and breadth is what
  says so.
- Impose **no earned-span floor**. The contracts axis requires a contract to span two
  packages because a single-package interface is local detail its package concept
  already covers. Nothing covers a table — it belongs to no package — so a floor would
  hide entities with no other home. Every table in the graph earns a concept.
- Mark each concept **internal or external**: a table some statement declares versus
  one only ever referenced. Which tables a repository uses but does not own is a
  question the axis can answer for free, and often the more interesting one.
- Put SELECTION in `atlas/tables.rs`, shared by the atlas producer and the query
  surface, taking input-agnostic projections. Render caps and concept-id slugs stay in
  the producer, where a cap is presentation policy that must never bound a query.
- Make the axis queryable as `kenn tables`, matching the other axes: a bare listing of
  flat scalar rows ranked by breadth, and a named lookup returning one table's
  references grouped by file.

Depends on tables existing in the graph. Until a SQL producer lands, the axis is
correct and empty.

## Capabilities

### Added Capabilities

- `atlas-tables` — the tables axis: table concepts, their references, and the query.
