## 1. Selection — one implementation

- [ ] 1.1 Add `crates/kenn-indexer/src/atlas/tables.rs` holding SELECTION only,
  modelled on `atlas/contracts.rs`: given input-agnostic projections of table nodes and
  their edges, return each table with its ownership mark, its reference sites grouped by
  file, and its rank. No render caps, no concept-id slugs — those stay in the producer,
  because a cap is presentation policy that must never bound a query. → verify: the
  module compiles without depending on the indexer's record types or the store's row
  types.
- [ ] 1.1a Project **per-site** edges, not rolled-up ones — unlike `atlas/contracts.rs`,
  which projects `aggregate_nodes` + `aggregate_edges`. Roll-up collapses a document's
  members into the document, so an aggregated projection would report a file where the
  graph knows an element and discard the attribution this axis exists to show. Each
  reference carries its site node, its file, its language, and which of `DefinesTable` /
  `AltersTable` / `AccessesTable` its edge is. → verify: two sites in one file
  referencing the same table are returned as two distinct references, not merged.
- [ ] 1.2 Rank by distinct referencing files, then distinct languages, then a stable
  tiebreak so a rebuild of the same snapshot orders identically. → verify: a
  many-file/many-language table outranks a single-file one; two runs over one snapshot
  produce the same order.
- [ ] 1.3 Mark each table internal when some statement declares it and external
  otherwise, and include external tables in the result. → verify: a table with only
  access references is returned and marked external.
- [ ] 1.4 Impose no earned-span floor — unlike `MIN_CONTRACT_PKGS`, a single-reference
  table still earns a concept, because no package concept covers it. → verify: a table
  referenced exactly once is present in the selection.

## 2. Producer — the concept documents

- [ ] 2.1 Emit one `table` concept per selected table in the atlas bundle
  (`crates/kenn-indexer/src/atlas/producer.rs`), with frontmatter carrying the
  ownership mark and the reference counts, and a body listing references grouped by
  file. → verify: a workspace with tables writes one concept per table with its
  references listed.
- [ ] 2.2 Add the axis to the bundle index alongside the existing axes, ranked by
  breadth. → verify: `index.md` lists the tables axis with per-table reference counts.
- [ ] 2.3 Keep render caps producer-side and report pre-cap totals so a capped
  document does not read as complete. → verify: a workspace exceeding the cap renders
  the cap's worth while reporting the true total.
- [ ] 2.4 Emit the axis without the analysis pass, as the contracts axis does. →
  verify: building with clustering disabled still writes table concepts (spec
  scenario).
- [ ] 2.5 Emit an empty axis rather than failing or omitting it when the graph holds
  no tables. → verify: a workspace with no SQL producer builds an atlas whose tables
  axis is present and empty (spec scenario).

## 3. Query surface

- [ ] 3.1 Add a `kenn tables` subcommand matching the other axis commands: a bare
  listing of flat scalar rows ranked by breadth with the resolvable id first, and
  `--all` for every row. → verify: the bare listing renders as a header-once table.
- [ ] 3.2 Naming a table returns its references grouped by file, each identifying the
  statement and whether it declares, modifies, or accesses the table. → verify: a named
  table returns grouped references with their roles.
- [ ] 3.3 Treat the name argument as a query over display name or resolvable id, and
  return every match tagged by id rather than erroring when it resolves to several. →
  verify: an ambiguous name returns all matches and no error.
- [ ] 3.4 Drive the query from `atlas/tables.rs`, projecting the store's rows into the
  same shared input type the producer projects its records into. → verify: producer and
  query agree on identities, ownership, and ordering for one snapshot (spec scenario);
  a workspace with more tables than the render cap still reaches every table by query.
- [ ] 3.5 Expose the axis through the MCP surface alongside the other axes. → verify:
  the tool returns the same rows as the CLI for one snapshot.

## 4. Verification

- [ ] 4.1 Index a real repository with SQL in `./tmp` and read the tables axis. →
  verify: table counts match the schema the repository declares and references; a table
  referenced from more than one file lists all of them.
- [ ] 4.2 Mutation-check the no-floor rule (§9): impose a two-file minimum, confirm the
  single-reference test goes red, restore. → verify: red on mutation, green after.
- [ ] 4.3 Mutation-check the cap/query separation: let the render cap bound the query,
  confirm the reachability test goes red, restore. → verify: red on mutation, green
  after.
- [ ] 4.4 Mutation-check the single-implementation rule: give the query its own ranking
  copy, confirm the producer/query agreement test goes red, restore. → verify: red on
  mutation, green after.
- [ ] 4.5 `cargo clippy --workspace --all-targets` clean, `just crap-ci` green, then
  `cargo fmt --all`, then clippy once more (§7 ordering).
