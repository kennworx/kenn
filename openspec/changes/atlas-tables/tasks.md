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
  the tool returns the same rows as the CLI for one snapshot. **Deferred to
  `split-query-from-mcp` §6.1, with the reason corrected.** The blocker was never this
  axis: NO atlas axis is registered as an MCP tool, though `list_packages`,
  `list_domains`, `list_contracts`, and `list_tables` all exist in `kenn-mcp/src/tools/`
  and are proven by their CLI verbs. Registering tables alone would put it where no
  sibling lives; registering all four is a decision about the MCP surface, which that
  change makes cheap by turning a query into a pure function over a snapshot.

## 4. Verification

- [x] 4.1 Index a real repository with SQL in `./tmp` and read the tables axis. →
  verify: table counts match the schema the repository declares and references; a table
  referenced from more than one file lists all of them.
- [x] 4.2 Mutation-check the no-floor rule (§9): impose a two-file minimum, confirm the
  single-reference test goes red, restore. → verify: red on mutation, green after.
- [x] 4.3 Mutation-check the cap/query separation: let the render cap bound the query,
  confirm the reachability test goes red, restore. → verify: red on mutation, green
  after. **The fixture was the work.** Against the single-table workspace next door
  every cap mutation passes, because one table and two references never reach a cap —
  the check would have guarded nothing while reading as a pass (§9: suspect the fixture
  before the test). `wide_table_workspace` is built past all three: 45 tables >
  `MAX_TABLES` 40, `t45` named from 15 files > `MAX_TABLE_FILES` 12, and 8 of those
  references in one file > `MAX_REFS_PER_FILE` 6. `take(40)` in the query → 40 against
  45, red. `take(12)`/`take(6)` on the sites → 17 against 22, red. Pre-cap `file_span`
  and `references` stayed correct under both, which is the point: a truncated list with
  honest totals is what reads as complete.
- [x] 4.4 Mutation-check the single-implementation rule: give the query its own ranking
  copy, confirm the producer/query agreement test goes red, restore. → verify: red on
  mutation, green after. Sorting the query's items by name → `t01` where breadth ranks
  `t45`, red. This one is only a guard because `t45` is LAST alphabetically and FIRST by
  breadth: had the widest table been `t01`, the mutation would have produced the right
  answer for the wrong reason and survived.
- [x] 4.5 `cargo clippy --workspace --all-targets` clean, `just crap-ci` green, then
  `cargo fmt --all`, then clippy once more (§7 ordering). All four green; `just test` 60
  suites, 0 failures. The new fixture helpers are not `#[test]` fns, so
  `allow-unwrap-in-tests` does not cover them — `unwrap`/`panic`/`format_collect` all
  fired and were fixed rather than allowed.

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
packages, domains, contracts, or documents. They are all CLI-only, and the 35
registered tools are symbol-level. Registering tables alone would put it
somewhere no sibling axis lives. Either every axis goes onto MCP or none does,
and that is a decision about the MCP surface rather than about this axis.

Revisited: the four axis *tools* already exist — `list_packages`,
`list_domains`, `list_contracts`, `list_tables` all live in
`kenn-mcp/src/tools/`, return `ListResponse<T>` over `JsonSchema` args, and are
exercised daily by their CLI verbs. Only the `#[tool]` wrapper is missing. What
made that feel like a layering question is that the crate holding them is 87%
not MCP, which is what `split-query-from-mcp` addresses; 3.5 moves there as
§6.1.

**Gate.** The new arm pushed the CLI router over threshold, and splitting it by
category made things *worse*: the extracted `dispatch_axis` came out at 0%
coverage, which revealed that the router's 45% had been entirely on the non-axis
arms — no test had ever run an axis command. Fixed by covering them:
`the_atlas_axes_answer_on_a_built_snapshot` walks all five, and
`naming_a_table_returns_its_reference_sites` covers the drill-in.

That test also caught a self-inflicted break: the `kenn init` template now ships
a `[language.sql]` section, so appending one is a duplicate-key parse error.

**Done — 4.3, 4.4, and the fixture was the whole difficulty.** Both rules held by
construction, but "holds by construction" is what every surviving mutation says
about itself. Pinning them needed a workspace wide enough for a cap to bite: the
single-table fixture the axis shipped with cannot fail a cap check, because one
table and two references never reach `MAX_TABLES` 40, `MAX_TABLE_FILES` 12, or
`MAX_REFS_PER_FILE` 6. Every mutation would have passed, and the pass would have
been meaningless.

`wide_table_workspace` is built past all three — 45 tables, one named from 15
files, 8 of those references in a single file — and one detail in it is
load-bearing: `t45` is last alphabetically and first by breadth. Rank the query
by name instead of by the shared selection and it answers `t01`; had the widest
table been `t01`, the same mutation would have produced the right answer for the
wrong reason and survived silently.

Three mutations, three reds, each for its stated reason:

| mutation | got | wanted |
|---|---|---|
| `take(40)` on the query's table list | 40 | 45 |
| `take(12)` files / `take(6)` sites | 17 | 22 |
| query re-sorts by name | `t01` | `t45` |

Under the cap mutations `file_span` and `references` stayed correct at 15 and 22
— the totals are pre-cap by construction. That is exactly the failure worth
guarding: a truncated list under an honest total is what reads as complete.
