## 1. Placement

- [x] 1.1 Add the pass as a post-code barrier step in
  `crates/kenn-indexer/src/pipeline/api.rs`, modelled on `resolve_css_usage_unit`. Both
  inputs exist once phase 1 has joined: code symbols with their body extents, and the
  table nodes the `.sql` producer wrote. → verify: a workspace with code but no `.sql`
  still runs the step (it mints external tables); one with neither skips it cleanly.
- [x] 1.2 Isolate failures into the step's own `RunReport`, leaving the producers' output
  intact. → verify: a forced failure degrades only this step's report.

## 2. Attribution — the load-bearing rule

- [x] 2.0 Add a bulk reader method for `(symbol short id, file, body extent)` — the trait
  offers `fetch_def_lines` per symbol and `scan_def_files` without lines, and the
  candidate set is every symbol with a body, so per-symbol round-trips are not viable. A
  sibling of the existing `scan_*` family. → verify: the method returns only symbols with
  a usable extent; an empty workspace returns empty rather than erroring.
- [x] 2.0a Provide the store-backed `TableRegistry` implementation, or reuse
  `xml-sql-bridge`'s if that landed first — only the in-memory `NameSet` exists today,
  and both changes need to resolve against identities read back from the store. Do NOT
  build a second one: that is the `css/usage.rs` + `html/classes.rs` duplication the
  registry requirement exists to prevent. → verify: a workspace search finds exactly one
  store-backed implementation.
- [x] 2.1 Build, per file, the list of `(symbol, body extent)` pairs from the store, and
  attribute each literal to the symbol whose extent is the SMALLEST one containing it.
  This is the rule the spike exists to have found: extents nest, so a module's span
  contains its functions' and a class's contains its methods'. Measured, per-symbol
  slicing gave `rs:kenn-collect::gc` (the module) the full table set of `Store::gc`
  inside it. → verify: a function inside a module references the table and the module
  does not (spec scenario); a method inside a class inside a module is the only one
  (spec scenario).
- [x] 2.2 Scan each file ONCE, not once per symbol. Per-symbol slicing re-reads the same
  bytes once per enclosing scope, which is both wasted work and the shape that produces
  the duplicate attribution 2.1 forbids. → verify: a file with N nested symbols is read
  once; the emitted edge count matches the innermost attribution.
- [x] 2.3 Drop a literal that falls outside every recorded extent. → verify: no reference
  is emitted for it (spec scenario); it is not attributed to the file.
- [x] 2.4 Mutation-check attribution: attribute to every containing symbol instead of the
  innermost, confirm the module test goes red, restore. This is the single guard worth
  the most here — without it the graph still populates, plausibly, and wrongly. →
  verify: red on mutation, green after.

## 3. Literal recovery

- [x] 3.1 Add a per-language literal scanner over file text, starting with the languages
  that carry body extents today — measured on a self-index: rust 8613, csharp 511,
  typescript 204. → verify: each scanner recovers a plain literal's contents as written.
- [x] 3.2 Cover each language's raw/verbatim forms: Rust `r"…"` / `r#"…"#`, C# `@"…"` and
  raw `"""…"""`, TypeScript template literals. SQL is written in exactly these forms
  precisely because it contains quotes and newlines, so a scanner that handles only the
  plain form misses the queries most worth finding. → verify: a raw-string query is
  recovered intact; escapes do not corrupt it (spec scenario).
- [x] 3.3 Contribute nothing for a language with no scanner, and report no failure. →
  verify: an unsupported language's files are silent (spec scenario).

## 4. Extraction and edges

- [x] 4.1 Pass each literal to the shared extractor (`crates/kenn-indexer/src/sql/parse.rs`)
  unchanged. Do NOT add a second parser, a second dialect strategy, or a code-specific
  normalization. → verify: identical text in a `.sql` file and a code literal yields
  identical references and grades (spec scenario).
- [x] 4.2 Require a COMPLETE parse of the literal, and do NOT apply the whole-then-split
  tiering the `.sql` producer uses — a literal is one fragment, not a file of statements,
  and splitting it manufactures exactly the partial parses where a query-local name gets
  read as a table (the CTE defect scanning real source turned up). Same rule
  `xml-sql-bridge` applies to element text. → verify: a query fragment contributes
  nothing (spec scenario); a literal holding two statements still yields both.
- [x] 4.2a Share the minted-table id allocation with any other barrier step that mints.
  Both this step and `xml-sql-bridge` mint external tables into the same `Sql` `ShortId`
  partition, continuing past what the `.sql` pass burned — two steps allocating
  independently from the same high-water mark collide, producing two symbols with one id.
  → verify: a workspace exercising both steps emits no duplicate short id; mutation —
  give each step its own allocator and confirm the collision test goes red.
- [x] 4.3 Emit `DefinesTable` / `AltersTable` / `AccessesTable` by what the statement
  does, sourced at the attributed code symbol, resolved through the SAME matching rule
  and registry the `.sql` producer uses, minting external on no match. → verify: a
  function querying a table reaches the node a `.sql` reference reaches (spec scenario);
  DDL in code marks the table declared (spec scenario).
- [x] 4.4 Treat an unparseable literal as ordinary text: no reference, no reported
  failure. Measured, 4103 bodies carried literals and 154 yielded a table, so a failure
  count here would report a defect on 97% of the corpus. → verify: log messages, paths,
  and format strings are silent (spec scenario).

## 5. Reporting and filtering

- [x] 5.1 Report bodies scanned, bodies carrying literals, and references emitted, so a
  reader can distinguish a table no code touches from one whose access was not visible.
  → verify: the report carries all three (spec scenario). Landed as `def_bodies_seen`
  (exact fit — definitions carrying a body extent), a new `bodies_with_literals`, and
  `edges_seen`. `files_seen` stays 0 on purpose: it rolls up into the per-language file
  total `kenn index` prints (`cmd_index/core.rs`), so borrowing it double-counts files
  another unit already reported. Mutation: swapping two counts goes red.
- [x] 5.1a Do NOT carry a `test` flag on the reference itself. The referencing symbol's
  own row holds it and `RowNarrow` filters on it, exactly as for every other edge kind —
  a flag on the reference could only ever prove itself, which is what the first version's
  test did. → verify: measured on a self-index, `list usages sql:sessions` returns 8 and
  `--include-tests` returns 19, every added row `test=true`.
- [x] 5.2 Honour the workspace's existing test-inclusion setting. Fixtures create and
  drop tables freely — the spike surfaced `orders`, `public.users`, `junk`, and `report`
  from test bodies alone. → verify: a table referenced only from tests is excluded when
  tests are (spec scenario).

## 6. Verification

- [x] 6.1 Re-run the spike's numbers against the implementation on this repo and compare:
  9309 bodies scanned, 4103 with literals, 154 yielding tables, 41 tables, 356 edges —
  minus the module-level duplicates 2.1 removes. A materially higher edge count means
  attribution regressed. → verify: both figures recorded. Measured on this repo:
  9467 bodies scanned, 3315 with literals, 261 references, 40 tables — against the
  spike's 9309 / 4103 / 356 / 41. Edges down 27% (innermost attribution), tables down
  one (the CTE fix). The literal count fell because the spike counted a body once per
  enclosing scope; bodies scanned rose with the workspace.
- [x] 6.2 Run against a real database-backed repository per language, not just kenn —
  kenn is a code tool whose SQL is its own storage layer, which is an unusual shape. →
  verify: coverage recorded per repo. Two private services, described by shape only:

  | workspace | bodies | w/ literals | refs | tables |
  |---|---|---|---|---|
  | Rust + Postgres, multi-schema, sqlx | 2206 | 1082 | 377 | 13 minted (49 total) |
  | C# + TypeScript + Postgres | 10497 | 4465 | 299 | 71 minted (74 total) |

  Both resolve schema-qualified names correctly across four schemas. No ORM-only
  workspace was in the set, so that arm of the requirement is still unverified.

  **This run found a crash.** A C# interpolated string reached the store as the table
  `sql:{schema.Value}` and aborted the run on the shell-safe-`pub_id` invariant. Two
  independent defects behind it, fixed separately: the shared extractor treated an
  interpolation placeholder as a table name, and neither SQL producer floored its
  `pub_id` the way every other producer does. The self-index could not have surfaced
  either — kenn's own SQL is static and its identifiers are clean.
- [x] 6.3 Spot-check precision by hand on a sample of emitted edges. The spike's apparent
  false positives (`n`, `f`, `t`) were all genuine short-named tables, and the one real
  defect was a CTE — precision claims here need reading the source, not eyeballing the
  name. → verify: a sample is checked against source. Every name below was read back to
  its statement. Recall looks good; precision has one systematic defect, recorded as
  6.3a rather than fixed here.

  True positives that *look* wrong: system catalogs (`pg_class`, `pg_namespace`,
  `information_schema.columns`) and migration-tracking tables are genuinely queried by
  this code — 22 of one workspace's 74. **Reported as-is, deliberately.** Filtering them
  would mean carrying a list of one database's internal names inside a module whose whole
  contract is "SQL text in, references out": Postgres-specific in a 14-dialect sweep,
  drifting with every server version, and wrong about the fact anyway — the code really
  does read those tables. kenn analyses SQL, not databases. Do NOT add a catalog filter
  here; a caller that wants to hide them can filter on the name it already has.
- [x] 6.3a **Not everything a statement names is a table.** Two independent causes, and
  the larger one was not the relation walk at all:

  1. **`DROP` ignored its own `object_type`.** Every name in a `Drop` became a table, so
     `DROP INDEX`, `DROP ROLE`, `DROP SCHEMA` and `DROP TYPE` all minted one. This was 12
     of the 15 bad identities. Now gated to `Table | View | MaterializedView` — views
     stay, because code selects from one exactly as from a table.
  2. **A function call in `FROM` reaches the relation walk as a bare name.**
     `FROM generate_series(1, 2000)`, `FROM tenant.uses_key($1, $2)`. `TableFactor::Table`
     carries `args`, `Some` exactly when the source wrote a call, so those names are
     collected through `pre_visit_table_factor` and subtracted from the walk. The
     distinction cannot be made after the fact — the walk is handed the `ObjectName`
     alone.

  → verify: re-measured on both workspaces, **every change a removal, none an addition**:

  | workspace | tables before | after | removed |
  |---|---|---|---|
  | C# + TypeScript | 74 | 59 | 9 index/constraint names, 3 roles, one enum type, one schema, one function |
  | Rust | 49 | 45 | 2 roles, 2 functions |

  Every removal was read back to its statement and confirmed — the Rust workspace's four
  are exactly the four the 6.3 hand-check had flagged. Mutation: each cause reverted
  independently goes red on its own test. Applies to the `.sql` producer identically,
  since both share the extractor.
- [x] 6.4 Confirm the step stays cheap enough to need no scoping control. Measured, the
  spike scanned 9309 bodies — including the full 14-dialect sweep on every literal that
  is not SQL — in **1.71s** release. That is why this change specifies no `roots`
  equivalent to `xml-sql-bridge`'s: there is nothing to bound. Re-measure rather than
  assume it still holds. → verify: the step's wall time is recorded; a scoping control is
  added only if it stops being negligible. Re-measured across three workspaces:

  | workspace | bodies | step |
  |---|---|---|
  | kenn | 9481 | 3s |
  | Rust service | 2206 | 4s |
  | C# + TypeScript | 10497 | 2s |

  Cost tracks the dialect sweep on literals that are *not* SQL, not body count — which
  is why the smallest workspace was the slowest. So instead of a scoping control, a
  keyword pre-filter now gates the parser: text naming no statement verb cannot yield a
  table and never reaches the sweep. Rust 4s → **0s**, C#/TS 2s → **1s**.
- [x] 6.4a Choose the pre-filter's keyword set by measurement, not by argument. The final
  set is the nine verbs that name a table and are not subsumed: `select insert update
  delete upsert create alter drop truncate`. Two kinds of omission, and the second is the
  one that matters:

  - **Subsumed by another verb in the same text** — `with` (always resolves to a
    read/write statement), `merge` (carries `UPDATE`/`INSERT` in its `WHEN` branches),
    `replace` (a scalar function, or part of `CREATE OR REPLACE`).
  - **Subsumed by another statement on the same symbol** — `lock` and `analyze`. Each
    names a table with no other verb, so the literal genuinely is skipped. But a symbol
    that locks or analyses a table also reads or writes it — that is *why* it locked it.
    Measured both ways on a workspace with `LOCK TABLE` in 8 files: **169 distinct edges
    either way, identical**.

  The distinction is worth keeping: what matters is whether an *edge* disappears, not
  whether a literal is skipped. A first pass restored `lock` and `analyze` on
  literal-level evidence and would have paid for two keywords that bought nothing.

  Added `audit_prefilter_false_negatives`, an `#[ignore]`d instrument (not a guard) that
  walks a real workspace and reports every rejected literal the parser would have
  accepted. It reports literals, so it *overstates* the loss — re-index and diff the edge
  set before adding a keyword back.

  → verify: 46625 literals rejected in the largest workspace and no distinct edge lost.
  The residual rejections are bare `from …` fragments inside test string-assertions,
  which were false positives all along: sqlparser accepts a bare `FROM x` as a statement,
  so the English phrase `"From Root Task"` had been minting a table named `Root`.
- [x] 6.5 `cargo clippy --workspace --all-targets` clean, `just crap-ci` green, then
  `cargo fmt --all`, then clippy once more (§7 ordering). First run failed on
  `ingest_code_tables` (CRAP 34.8, cyclomatic 11, coverage 41.8%). Fixed by extracting
  the pure `known_tables` — the only part of that module that decides anything — which
  moved both terms: cyclomatic 11→8 and coverage up, since a pure helper takes a direct
  unit test where the orchestrator only gets incidental integration coverage. Baseline
  untouched, still empty.
