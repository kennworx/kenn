## 1. Selection — one implementation

- [x] 1.1 Add `crates/kenn-indexer/src/atlas/tables.rs` holding SELECTION only,
  modelled on `atlas/contracts.rs`: given input-agnostic projections of table nodes and
  their edges, return each table with its ownership mark, its reference sites grouped by
  file, and its rank. No render caps, no concept-id slugs — those stay in the producer,
  because a cap is presentation policy that must never bound a query. → verify: the
  module compiles without depending on the indexer's record types or the store's row
  types.
- [x] 1.1a Project **per-site** edges, not rolled-up ones — unlike `atlas/contracts.rs`,
  which projects `aggregate_nodes` + `aggregate_edges`. Roll-up collapses a document's
  members into the document, so an aggregated projection would report a file where the
  graph knows an element and discard the attribution this axis exists to show. Each
  reference carries its site node, its file, its language, and which of `DefinesTable` /
  `AltersTable` / `AccessesTable` its edge is. → verify: two sites in one file
  referencing the same table are returned as two distinct references, not merged.
- [x] 1.2 Rank by distinct referencing files, then distinct languages, then a stable
  tiebreak so a rebuild of the same snapshot orders identically. → verify: a
  many-file/many-language table outranks a single-file one; two runs over one snapshot
  produce the same order.
- [x] 1.3 Mark each table internal when some statement declares it and external
  otherwise, and include external tables in the result. → verify: a table with only
  access references is returned and marked external.
- [x] 1.4 Impose no earned-span floor — unlike `MIN_CONTRACT_PKGS`, a single-reference
  table still earns a concept, because no package concept covers it. → verify: a table
  referenced exactly once is present in the selection.

## 2. Producer — the concept documents

- [x] 2.1 Emit one `table` concept per selected table in the atlas bundle
  (`crates/kenn-indexer/src/atlas/producer.rs`), with frontmatter carrying the
  ownership mark and the reference counts, and a body listing references grouped by
  file. → verify: a workspace with tables writes one concept per table with its
  references listed.
- [x] 2.2 Add the axis to the bundle index alongside the existing axes, ranked by
  breadth. → verify: `index.md` lists the tables axis with per-table reference counts.
- [x] 2.3 Keep render caps producer-side and report pre-cap totals so a capped
  document does not read as complete. → verify: a workspace exceeding the cap renders
  the cap's worth while reporting the true total.
- [x] 2.4 Emit the axis without the analysis pass, as the contracts axis does. →
  verify: building with clustering disabled still writes table concepts (spec
  scenario).
- [x] 2.5 Emit an empty axis rather than failing or omitting it when the graph holds
  no tables. → verify: a workspace with no SQL producer builds an atlas whose tables
  axis is present and empty (spec scenario).

## 3. Query surface

- [x] 3.1 Add a `kenn tables` subcommand matching the other axis commands: a bare
  listing of flat scalar rows ranked by breadth with the resolvable id first, and
  `--all` for every row. → verify: the bare listing renders as a header-once table.
- [x] 3.2 Naming a table returns its references grouped by file, each identifying the
  statement and whether it declares, modifies, or accesses the table. → verify: a named
  table returns grouped references with their roles.
- [x] 3.3 Treat the name argument as a query over display name or resolvable id, and
  return every match tagged by id rather than erroring when it resolves to several. →
  verify: an ambiguous name returns all matches and no error.
- [x] 3.4 Drive the query from `atlas/tables.rs`, projecting the store's rows into the
  same shared input type the producer projects its records into. → verify: producer and
  query agree on identities, ownership, and ordering for one snapshot (spec scenario);
  a workspace with more tables than the render cap still reaches every table by query.
- [ ] 3.5 Expose the axis through the MCP surface alongside the other axes. → verify:
  the tool returns the same rows as the CLI for one snapshot.

## 4. Verification

- [x] 4.1 Index a real repository with SQL in `./tmp` and read the tables axis. →
  verify: table counts match the schema the repository declares and references; a table
  referenced from more than one file lists all of them.
- [x] 4.2 Mutation-check the no-floor rule (§9): impose a two-file minimum, confirm the
  single-reference test goes red, restore. → verify: red on mutation, green after.
- [ ] 4.3 Mutation-check the cap/query separation: let the render cap bound the query,
  confirm the reachability test goes red, restore. → verify: red on mutation, green
  after.
- [ ] 4.4 Mutation-check the single-implementation rule: give the query its own ranking
  copy, confirm the producer/query agreement test goes red, restore. → verify: red on
  mutation, green after.
- [ ] 4.5 `cargo clippy --workspace --all-targets` clean, `just crap-ci` green, then
  `cargo fmt --all`, then clippy once more (§7 ordering).

## What landed, and what did not

**Done: selection (1.x) and the query surface (3.1–3.4).**

`atlas/tables.rs` holds selection only — caps and slugs stay with the caller, as
in `atlas/contracts.rs`. References are projected **per site** rather than rolled
up: a table's references have no package to roll into, and which *file* made the
reference is the whole answer to "what touches this, and where". Ranking is
reference **breadth** — distinct files, then distinct languages, then total, then
name — because a table named by a migration, a changelog and application code is
the architecturally interesting one, and a hundred reads from one file is not.

No earned-span floor, unlike `MIN_CONTRACT_PKGS`. A single-package contract is
covered by its package concept; a single-file table is covered by nothing,
because nothing else in the atlas is organised around tables.

`kenn tables` measured on a real Postgres repository:

```
items[25]{symbol,name,internal,file_span,language_span,references}:
  "sql:ledger.accounts",accounts,true,19,2,50
  "sql:ledger.postings",postings,true,13,2,40
```

and `kenn tables sql:ledger.accounts` returns every site grouped by file, each
carrying its kind, language, file and symbol name.

**Mutation checked:** imposing a two-file floor turns four selection tests red,
including the no-floor one.

**Done: the bundle concepts (2.x).** The atlas now writes a `tables/` directory
and a `## Tables` section. Measured on a real Postgres repository:

```
## Tables — 51, heaviest 40 shown · all via `kenn tables`

- [accounts](/tables/accounts.md) — 51 references across 19 files
- [postings](/tables/postings.md) — 41 references across 12 files
```

and each concept doc carries the table's `pub_id`, whether the workspace
declares it, and its references grouped by file with what each one does:

```
## References — 51 across 19 files in 2 languages, heaviest 12 shown

### `crates/ledger/tests/scoping.rs` — 7 (rust)
| Does | ID | Location |
| accesses | `rs:…::fixture` | …/scoping.rs:30-82 |
```

Pre-cap totals in the heading and the `all via \`kenn tables\`` pointer, so a
capped axis never reads as complete (2.3). Rendered from the same
`atlas/tables.rs` selection the query uses, with no analysis pass (2.4), and two
tables sharing a name de-duplicate to `events.md` / `events-2.md`.

Raw per-site edges are scanned for this, not aggregate ones — an aggregate has
already collapsed the referencing file, which is the axis's answer.

**NOT done — 3.5, MCP exposure, and its premise is wrong.** The task says
"alongside the other axes", but **no atlas axis is on the MCP surface**: not
packages, domains, contracts, or documents. They are all CLI-only, and the MCP
tools are symbol-level. Registering tables alone would put it somewhere no
sibling axis lives. Either every axis goes onto MCP or none does, and that is a
decision about the MCP surface rather than about this axis.

**Gate.** The new arm pushed the CLI router over threshold, and splitting it by
category made things *worse*: the extracted `dispatch_axis` came out at 0%
coverage, which revealed that the router's 45% had been entirely on the non-axis
arms — no test had ever run an axis command. Fixed by covering them:
`the_atlas_axes_answer_on_a_built_snapshot` walks all five, and
`naming_a_table_returns_its_reference_sites` covers the drill-in.

That test also caught a self-inflicted break: the `kenn init` template now ships
a `[language.sql]` section, so appending one is a duplicate-key parse error.

**NOT done — 4.3, 4.4.** The cap/query separation and single-implementation rules
hold by construction (the query never applies a cap, and it calls
`atlas::tables::select_tables`), but neither is pinned by a test that would go
red, so neither is claimed.
