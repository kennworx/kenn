## 1. Model

- [x] 1.1 Add `Language::Sql` (`crates/kenn-model/src/language.rs`) claiming `sql`,
  with `id_prefix`/`display`/`extensions` arms and the `from_str` round trip. →
  verify: `Language::from_ext("sql")` resolves; every existing `match` on `Language`
  still compiles exhaustively.
- [x] 1.2 Add `DefinesTable`, `AltersTable`, and `AccessesTable` to `EdgeKind`
  (`crates/kenn-model/src/edge.rs`), each with its relation-table name. `CREATE` and
  `ALTER`/`DROP` are separate kinds, not one: a table's definition is a fold over many
  statements, so "what created this" and "what has changed it" must be separately
  walkable — collapsing them makes the change's motivating question unanswerable. →
  verify: `EdgeKind::name()` round-trips all three; the DB schema-drift test sees the
  new relation tables.
- [x] 1.3 Add `Kind::SqlTable` and `Kind::SqlStatement`
  (`crates/kenn-model/src/kind.rs`) — dedicated variants following the
  `CssClass`/`CssId`/`HtmlId` precedent, prefixed because markdown and HTML tables
  are plausible future node kinds. Touch only `ALL`, `db_name`, and `from_db_name`:
  both are `false` for `is_scope`, `is_class_like`, and `is_callable`. Reusing a code
  kind is wrong — `is_class_like` drives nearest-enclosing aggregate-id rollup, so a
  table borrowing `Struct` would enter package-level rollup as a type. One statement
  kind covers every statement shape: a create-as-select both defines and accesses, so
  that role belongs on the edge, where it can be plural. → verify:
  `db_names_are_unique` still passes; both kinds round-trip `db_name`/`from_db_name`;
  neither appears in any predicate set.
- [x] 1.4 Add `crates/kenn-model/src/id/sql.rs`: `table_id(schema: Option<&str>,
  name) -> PublicId` producing `sql:<table>` / `sql:<schema>.<table>`, plus
  `file_id` and `statement_id` for the file and per-statement nodes. → verify:
  qualified and unqualified ids differ; the same `(schema, name)` is stable across
  calls; ids contain no file path.

## 2. Config

- [x] 2.1 Add `SqlConfig` (`crates/kenn-config/src/language/sql.rs`) with a `dialect:
  Option<String>` field, wired into `LanguageConfig`, defaulting disabled, mirroring
  `CssConfig`'s shape. → verify: round-trips through config load; disabled by default;
  dialect defaults to unset.
- [x] 2.2 Register the `sql` extension claim alongside the existing per-language
  claims so an enabled `[language.sql]` claims `.sql`. → verify: the claimed-extension
  test asserts `sql` is claimed when enabled and absent when not.

## 3. Parser — pure module

- [x] 3.1 Add `crates/kenn-indexer/src/sql/parse.rs`: SQL text → statements, each with
  its byte span and the table references it makes, each classified as a definition, a
  modification, or an access. No file I/O, no store access, no knowledge of its caller.
  Back it with `sqlparser` (`apache/datafusion-sqlparser-rs`) — the only Rust parser
  covering Oracle/MSSQL/MySQL/PostgreSQL from one API, and lean (one mandatory dep,
  `log`; no arrow/datafusion despite the repo name). NOT tree-sitter: no multi-dialect
  SQL grammar exists and there is no maintained Oracle or T-SQL grammar. → verify: a
  fixture string yields the same statements and references as the identical text read
  from a file (spec scenario).
- [x] 3.2 Select the primary dialect from `[language.sql] dialect` via
  `dialect_from_str`, defaulting to `GenericDialect` when unset. Do NOT auto-detect:
  driver-manifest sniffing would vendor library knowledge, and trial-parse selection
  is subsumed by 3.3. → verify: an unset dialect indexes cross-dialect fixtures; an
  unknown dialect name is a config error, not a silent fallback.
- [x] 3.3 On any parse failure, sweep the remaining dialects in a fixed order and keep
  the first that parses. This applies to **every parse attempt at either tier** of
  3.4 — the whole-file attempt and each split piece — not only to statements.
  Measured per statement (`tmp/sqlspike` v2): generic alone 12/16, generic + sweep
  15/16, better than any fixed dialect (best single was mssql 13/16), at ~71µs per
  statement amortized since the sweep only fires on failures. Do NOT prefilter
  candidates by syntax markers: they misroute — `(+)` is Oracle's outer-join operator
  yet only `mssql` parses it, and a PL/SQL `BEGIN` block is likewise rescued by
  `mssql`, not `oracle`. → verify: a bracket-quoted statement the primary rejects is
  recovered; the recovering dialect is deterministic for a given input.
- [x] 3.4 Parse each file **whole** first, and only if every dialect fails, split on
  top-level semicolons (public `Tokenizer`, `TokenWithSpan`/`Span`) and parse each
  piece. Splitting first loses data: measured per file (`tmp/sqlspike` v3), a file with
  a PL/SQL block parses whole under `mssql` yielding `{accounts, ledger, stale_rows}`,
  but naive splitting shears `BEGIN UPDATE accounts …` into a failing fragment and
  drops `accounts`. Splitting is still required, because a block whose body has no
  internal semicolons fails whole-file under every dialect while the `CREATE TABLE`
  after it splits out cleanly. The two-tier order was the maximum on all four measured
  files. → verify: the PL/SQL file yields three tables, not two; the
  no-inner-semicolon file still yields the table declared after the block.
- [x] 3.5 Drop split fragments that parse but reference nothing — a bare `END` parses
  as a statement with zero relations and would otherwise mint a meaningless statement
  node. → verify: splitting a block body produces no `END` node.
- [x] 3.6 Normalize extracted identifiers — strip dialect quoting (`` `users` ``,
  `[dbo].[users]`, `"users"`) before registry lookup or node minting. sqlparser
  returns names **with** their quoting, so unnormalized `` `users` `` and `users`
  mint two table identities, and a quoted reference silently drops under the
  no-stub rule instead of linking. → verify: DONE —
  `dialect_quoting_is_stripped_so_one_table_has_one_identity` covers all three quoting
  styles; mutation (drop the trims from `normalize_ident`) goes red with
  `left: ["`users`"] right: ["users"]`, and takes the dialect-sweep test with it.
- [x] 3.7 Resolve table aliases within a statement so `FROM users u JOIN orders o`
  yields `users` and `orders`, not `u` and `o`. → verify: alias-heavy statement
  yields exactly the two real tables.
- [x] 3.7a Exclude the names a `WITH` clause introduces, at any nesting depth — a CTE
  is query-local, and the relation visitor cannot distinguish `WITH cnt AS (…) SELECT
  FROM cnt` from a table read. Found by scanning real source: kenn's own
  recursive-CTE query minted a `cnt` table. Scope it to UNQUALIFIED names only, so a
  `WITH users AS (…)` cannot shadow a real `public.users`. → verify: a CTE-only query
  names no table; the CTE's own sources and the surrounding reads survive; a
  schema-qualified name of the same spelling is unaffected (spec scenario); mutation —
  disable the exclusion and confirm the tests go red.
- [x] 3.8 Mark references whose target is not statically knowable (runtime
  substitution) as unresolvable rather than emitting a name. → verify: a
  substituted table name produces an unresolvable reference, not a literal token.
- [x] 3.9 Re-run the whole strategy against a real repo with Oracle/T-SQL in `./tmp`
  before trusting it. Every number cited in 3.3–3.4 comes from `tmp/sqlspike`, which
  uses hand-written statements, not a corpus. Confirm on real source that tokenizing
  stays more permissive than parsing (3.4's split tier depends on it) and that
  whole-file-first still beats split-first. → verify: **the spike's advantage did not
  reproduce.** `audit_strategy_on_a_real_corpus` over two Postgres repositories (21 and
  54 `.sql` files) recovered *identical* table counts either way — 159/159 and 228/228,
  no file favouring either ordering. Whole-first stays the default because it is cheaper
  (one parse, not one per piece) and the shearing hazard is real where procedure blocks
  occur, but the coverage claim is now marked unproven outside the spike in the module
  doc. The tokenizer-more-permissive-than-parser property, which the split tier depends
  on, DID hold: 9 files in each repo failed a whole-file parse and still tokenized.
  **Neither corpus is Oracle or T-SQL** — the dialects where shearing is likeliest — so
  that half of this task remains unmeasured and is recorded as such.

## 4. Registry — one trait, one impl

- [x] 4.1 Add a `TableRegistry` trait (name → registered table identities) with the
  matching rule defined ONCE against the trait, not per consumer. Implementations may be
  more than one — the `.sql` pass resolves against the set it collected in memory, a
  later consumer against the same set read back from the store — but the rule may not.
  Do NOT mirror the `css/usage.rs` + `html/classes.rs` split, which declares the same
  lookup twice with no shared rule. → verify: a workspace search finds exactly one
  `TableRegistry` trait; two consumers backed by different identity sources return the
  same candidates and grades for the same name (spec scenarios).
- [x] 4.2 Implement the matching rule: a schema-qualified reference matches only the
  identity bearing that schema; an unqualified reference matches every known table of
  that name whatever schema it carries. Grade the result — one match → `Exact`;
  several → one `Ambiguous` edge per candidate; **no match → mint the table as
  external and link `Exact`**, never drop. A reference is dropped only when its name is
  not statically knowable. → verify: four-case unit test covering a qualified reference
  that must NOT match another schema, an unqualified reference reaching a
  schema-qualified table, the multi-candidate case keeping every candidate, and the
  no-match case minting an external table rather than dropping.

## 5. Producer — barrier-free unit

- [x] 5.1 Add `sql_phase1_unit` as a sibling ingest unit in
  `crates/kenn-indexer/src/pipeline/api.rs`, modelled on `text_unit` (barrier-free,
  no pending state). → verify: an SQL-only workspace indexes with no barrier step
  running.
- [x] 5.2 Parse every discovered file **once**, retain the parsed statements, then run
  two resolution passes over those results — never two parse passes, since parsing is
  the expensive step and a sweep may try every dialect. Pass 1 collects the full set of
  table names the workspace mentions — from declarations **and** from every other
  reference — and emits file, statement, and table nodes, marking a table internal when
  some statement declares it and external otherwise. Pass 2 resolves every reference
  against that set and emits graded edges. Building the name set from all references,
  not only declarations, is what lets an undeclared table still link. → verify: a query
  in a file that sorts *before* the file declaring the table resolves `Exact`; a table
  only ever queried is minted external and shared by its references; each file is
  parsed exactly once.
- [x] 5.3 Isolate per-file read and parse failures into the unit's `RunReport` and
  continue with the remaining files. → verify: one unusable file among several
  degrades the report and leaves the others indexed (spec scenario).

## 5a. Statement signature

- [x] 5a.1 Retain the parser's statement kind on `ParsedStatement`
  (`crates/kenn-indexer/src/sql/parse.rs`), set where `refs_of` already matches on
  `Statement`. The information is in hand there today and discarded — `ParsedStatement`
  carries only `span` and `refs`, so `UPDATE` and `SELECT` are indistinguishable
  downstream (both are `RefRole::Accesses`). → verify: two statements differing only in
  operation are distinguishable from the extraction alone. → verify:
  `the_verb_distinguishes_statements_the_role_cannot` shows `UPDATE` and `SELECT` sharing
  `RefRole::Accesses` while carrying different verbs.
- [x] 5a.2 Add a standalone `verb_of(&Statement) -> &'static str` mapping table — NOT a
  branch inside `refs_of`, which already carries enough complexity. Cover every
  table-bearing kind: `Query`, `Insert`, `Update`, `Delete`, `CreateTable`, `AlterTable`,
  `Drop` (rendering its `object_type`, since one variant covers table/view/index),
  `Truncate`, `Merge`, `RenameTable`, `CreateView`, `AlterView`, `CreateIndex`,
  `AlterIndex`, `CreateSchema`, `Analyze`, `OptimizeTable`, `Cache`, `UNCache`,
  `LockTables`, `Copy`, `LoadData`, `Unload`, `Comment`, `Grant`, `Revoke`. The enum has
  135 variants in sqlparser 0.62, but a statement naming no table produces no node, so
  the rest are unreachable from here. → verify: 22 arms walked one by one, and `DROP VIEW`
  / `DROP INDEX` each differ from `DROP TABLE`. `None` for an unmapped kind rather than a
  guess, pinned by `EXPLAIN SELECT`.
- [x] 5a.3 Cover `verb_of` with a test walking every arm. CRAP is
  `cyclomatic² × (1−coverage)³ + cyclomatic` against a threshold of 30, so a ~25-arm match
  scores 25 fully covered, 30 at 80% coverage, and 65 at 60% — this is the first function
  in the change that can trip the gate on its own. Expect
  `#[allow(clippy::match_same_arms)]` with a justification, which §5 sanctions for
  documentation-style mapping tables. → verify: gate green, and `verb_of` does not appear
  in the report at all — fully covered, it scores under threshold. **The predicted allow
  turned out to be unnecessary**: `#[expect(clippy::match_same_arms)]` came back
  *unfulfilled*, because every arm renders a distinct verb. That is a property worth
  keeping rather than a lint to silence, so the attribute was removed and the reason
  recorded on the function.
- [x] 5a.4 Render the signature in `ingest.rs` as verb plus the tables the statement
  names, signing a define-and-access statement by what it defines. No cap and no
  truncation. → verify: a four-table join names all four; `CREATE TABLE active AS SELECT
  … FROM users` signs `CREATE TABLE active`, and mutating that rule renders
  `CREATE TABLE active, users`. A self-join names its table once. Measured on a real
  repo: `SELECT FROM postings, balances, merchants`, `DROP TABLE bench_bare, bench_fk,
  bench_trig, bench_stmt`.
- [x] 5a.5 Fall back to the reference role for an unrecognized kind rather than emitting
  an empty signature. → verify: an unmapped kind signs `QUERY users`. The first version
  of this test asserted only `!s.is_empty()` and **survived the mutation** — the table
  list alone makes the string non-empty — so it now asserts on the verb, and the vacuous
  `if let` guard was replaced with an `expect` so a fixture that stops producing a
  statement fails loudly.

## 5b. Search surfaces

- [x] 5b.1 Extend the verbatim lexical projection in `build_name_rows`
  (`crates/kenn-store/src/db/sqlite/writer/finalize.rs`) to cover SQL alongside XML, and
  derive it from **both** surfaces so the statement text reaches the trigram index. Today
  the XML arm passes the signature through unsplit while everything else is
  identifier-split, and SQL statements carry an empty signature — so a statement's
  `name_text` is its synthetic name (`sql_statement …`) and its text is reachable only
  through the porter index. → verify: DONE by the shared verbatim arm landed in
  `index-xml` 4.6 — one arm covering both languages, keyed on `is_verbatim_language`.
  Measured on a real workspace: a column name a statement adds returns 3 substring hits,
  matching the 3 rows that contain it. (`VARCHAR(255)` does not occur in that corpus, so
  it proves nothing there — the column name is the real evidence.)
- [x] 5b.2 Exclude SQL from the embedding selection in `scan_rows`
  (`crates/kenn-store/src/db/jobs.rs`) alongside XML — one filter covering both, not two.
  SQL currently writes the statement to the content surface and is therefore embedded by
  default. → verify: a SQL-only workspace produces zero vectors after an embed pass (spec
  scenario); mutation-check by removing the filter and confirming vectors appear. →
  verify: DONE — one `n.language NOT IN (…)` filter covers both languages;
  `xml_and_sql_never_enrol_in_the_embedding_pass` and
  `an_xml_only_workspace_embeds_nothing_at_all` guard it, and neutralizing the filter
  turns both red naming the leaked rows.
- [x] 5b.3 Record the stopgap in the code where the projection is defined: statement text
  is verbatim-searchable because columns are not nodes, and this shrinks to a true
  signature when they are. → verify: DONE — the note sits on `verbatim_projection` in
  `kenn-store/src/db/codes.rs` and names the retirement condition (columns becoming
  nodes with their own identities), not just the current behaviour.

## 6. Verification

- [x] 6.1 A real repository with SQL migrations plus query files in `./tmp` indexes
  end to end; spot-check that one table node is reached from a `CREATE`, from an
  `ALTER`, and from a `SELECT` in different files. → verify: `kenn list usages`
  on the table id returns statements from every referencing file. → verify: `ledger.accounts`
  on a real Postgres repo — 23 usages across 6 files, reached by `CREATE TABLE accounts`,
  `ALTER TABLE accounts` (3 files) and reads (2 files), each carrying its rendered
  signature.
- [x] 6.2 Mutation-check the grading guard (§9): force `Ambiguous` resolution to
  return a single candidate, confirm the multi-candidate test goes red for that
  reason, restore. → verify: red — `an_unqualified_reference_matching_two_keeps_both`
  reports `left: 1 right: 2`.
- [x] 6.3 Mutation-check the drop rule: make a runtime-substituted name emit a table
  node, confirm the not-knowable test goes red, restore. → verify: red on both
  substitution guards — the `${…}` one and the quoted C# interpolation one.
- [x] 6.4 Mutation-check the mint rule: require a `DefinesTable` edge before a
  reference may link, confirm the undeclared-table test goes red, restore. This is the
  rule that decides whether a workspace whose schema lives elsewhere gets a graph at
  all — measured on a real repository, only 25 of 128 tables are declared in `.sql`. →
  verify: red on mutation, green after.
- [x] 6.5 `cargo clippy --workspace --all-targets` clean, `just crap-ci` green,
  then `cargo fmt --all`, then clippy once more (§7 ordering). → verify: gate green;
  `verb_of` does not appear in the report (fully covered, under threshold).
