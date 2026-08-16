## 1. Config

- [x] 1.1 Add a top-level `[xml_sql]` section (`crates/kenn-config/`), not nested under
  `[language.xml]` — the bridge is not a language, `XmlConfig` should not grow SQL
  concerns, and `Config` already carries non-language sections (`ingest`, `staleness`,
  `vectors`). → verify: round-trips through config load; absent section yields defaults.
- [x] 1.2 Give it `roots` and `dialect` with exactly `SqlConfig`'s shape and defaults —
  `roots: Vec<String>` defaulting to `["."]`, `dialect: Option<String>` defaulting to
  `None` (the permissive cross-dialect parse). Do NOT document `dialect` as a
  performance control: the permissive parse scored 13/16 against 10-11/16 for named
  dialects, so naming one is stricter, not faster or better informed. → verify: defaults
  are whole-workspace with no dialect (spec scenario); an unrecognized dialect name is a
  config error, never a silent fallback.
- [x] 1.3 Add the table-reference rules: each an attribute name, optionally an element
  name, and optionally a role (`declares` / `modifies` / `accesses`). Empty by default,
  so the bridge works from element text alone with no configuration. → verify: empty by
  default; a rule with an attribute but no element or role is valid.
- [x] 1.4 Reject a rule naming an element or role without an attribute — an element
  name alone identifies no table. → verify: such a rule is a config error naming the
  offending rule, not a silent no-op.

## 2. Barrier step

- [x] 2.1 Add the resolution as a barrier step in `crates/kenn-indexer/src/pipeline/api.rs`
  modelled on `resolve_css_usage_unit`, placed where both producers have joined
  (`api.rs:259-270`) rather than after the HTML step — neither input depends on code. It
  reads XML element nodes and the table registry from the building store; no producer
  carries pending state for it. → verify: both producers stay barrier-free; the step runs
  after both have joined.
- [x] 2.1a Add a bulk reader method for `(short_id, sig, doc)` filtered by language, a
  sibling of the existing `scan_*` family. The trait today offers only
  `fetch_symbol_docs_row(short_id)` — one row per call — and the candidate set is up to
  every element in the workspace, so per-symbol round-trips are not viable and the
  filtering belongs in SQL. → verify: the method returns only the requested language;
  a workspace with no XML returns empty rather than erroring.
- [x] 2.1b Select candidates with two disjoint filters: elements whose content is
  non-empty (the text arm — measured 1.8% of elements) and elements whose signature
  contains a configured attribute (the attribute arm). The signature prefilter is sound
  *because the producer renders it*: XML permits `tableName = "users"` with spaces, but a
  canonical rendering admits one form, so the prefilter cannot miss a match that parsing
  would have found. Parsing then confirms. → verify: an element written with spaces
  around the `=` is still selected; a prefiltered element that does not actually carry the
  attribute contributes nothing.
- [x] 2.2 Skip the step without error when the workspace has no XML elements or no
  table nodes. → verify: an XML-only and a SQL-only workspace both index cleanly with
  the step skipped (spec scenario). → verify: a pipeline test over XML that names no
  table indexes cleanly — the step succeeds with zero edges and mints nothing, and no
  unit degrades.
- [x] 2.3 Isolate step failures into its own `RunReport` and leave producer output
  intact. → verify: the step files an `xml-tables` report distinct from every producer's,
  and a reader/store error is caught into that report's `failed_projects` rather than
  propagating. NOT verified by a forced failure — the code path is structural (each arm's
  `Result` is captured independently, and the shared thread's panic maps to both), but no
  test drives it.

## 3. Element text → SQL

- [x] 3.1 For each candidate element, read its content surface and call the shared SQL
  extractor (`crates/kenn-indexer/src/sql/parse.rs`) — the same module the `.sql`
  producer uses, including its dialect recovery. Do NOT add a second extractor or a
  second dialect strategy. → verify: identical text in a `.sql` file and in an element
  yields the same references and grades.
- [x] 3.1a Confine the arm to the configured roots. The dialect sweep retries only a
  *failed* parse, so real SQL costs one permissive parse and the full sweep falls
  entirely on text that is not SQL — which is most element text. Roots remove that
  population rather than amortizing it. → verify: an element outside the roots
  contributes nothing; measure the step's wall time with roots at `["."]` and at the
  declared root, and record both rather than assuming the difference.

  **Half-verified.** The root filter is guarded by a test, and the whole-workspace run is
  measured (84169 elements). The narrowed-root comparison is NOT — the keyword pre-filter
  landed between the design of this task and its implementation, and it removes the same
  population the roots were meant to remove, so the two numbers may now be close. Measure
  before treating `roots` as a performance lever.
- [x] 3.2 Treat unparseable element text as "not SQL": contribute nothing, report
  nothing. Measured on a real repository only 1.8% of elements carry any text at all,
  and most of that is not SQL, so a failure count here would be noise rather than
  signal. → verify: elements holding versions, descriptions, and numbers produce no
  references and no reported failures.
- [x] 3.3 Emit declaring, modifying, and accessing edges according to what the parsed
  text does, exactly as the `.sql` producer does. → verify: an element whose text
  alters a table emits a modification edge; one that queries emits an access edge.

## 4. Attribute → table

- [x] 4.1 For each configured rule, emit a table reference from every element carrying
  the named attribute, using the attribute's value as the table name and applying the
  configured role when the element name matches. → verify: one configured attribute
  reaches tables named only by attributes (spec scenario); an unbound element carrying
  the same attribute emits a plain reference.
- [x] 4.2 Normalize attribute-derived names through the same identifier normalization
  the SQL path uses, so a quoted or schema-qualified attribute value resolves to the
  same identity a statement would produce. → verify: an attribute value carrying a
  schema qualifier resolves to the schema-qualified table, not a new one.
- [x] 4.3 Keep every framework vocabulary out of the implementation — no element name,
  attribute name, or namespace URI. → verify: a search of the bridge's source finds no
  framework vocabulary term (spec scenario).

## 5. Edges

- [x] 5.1 Set each edge's source to the XML element node that carried the reference,
  never the document. → verify: two elements in one document referencing different
  tables produce edges from their own elements (spec scenario).
- [x] 5.1a Share the store-backed `TableRegistry` and the minted-table id allocation with
  `code-table-references`, which needs both for the same reason. Only the in-memory
  `NameSet` exists today; whichever change lands first provides the store-backed
  implementation and the other reuses it. Two implementations of one lookup is the
  `css/usage.rs` + `html/classes.rs` duplication the registry requirement was written
  against. Allocation matters just as much: both steps mint into the same `Sql` `ShortId`
  partition past the `.sql` pass's high-water mark, and allocating independently produces
  two symbols with one id. → verify: exactly one store-backed implementation exists; a
  workspace exercising both steps emits no duplicate short id.
- [x] 5.2 Grade through the shared matching rule and registry used by the SQL producer
  — qualified matches its own schema only, unqualified matches every table of that
  name, several matches are all kept and marked ambiguous. Do NOT reimplement the rule.
  → verify: a bridged ambiguous reference keeps every candidate, matching the SQL
  producer's behaviour on the same registry.
- [x] 5.3 Mint an external table when a bridged reference matches nothing, as a `.sql`
  reference does. → verify: an element naming an undeclared table links to a minted
  external table (spec scenario); a `.sql` declaration and an XML reference to the same
  name reach one node.

## 6. Verification

- [x] 6.1 Index a real repository whose schema is declared by XML attributes and whose
  queries live in element text, with one configured attribute rule. → verify: measured on
  a private workspace of 1370 XML documents, with two rules (`tableName` bound to
  `createTable` as `declares`, and `tableName` unbound). 84169 elements scanned, 3045
  matched, **3774 references**, 85 tables minted. Tables reached from both surfaces at
  once, e.g. one with 9 `.sql` references and 30 from XML changesets — `kenn list usages`
  on it returns both, which is the join the change exists for.
- [x] 6.2 Confirm the measured gap closes. → verify: **48 tables without the bridge, 133
  with it** — 85 that no `.sql` file declares, close to the design's estimate of 25
  declared against 103 named by attribute. Most of that workspace's schema was invisible
  before this step.
- [x] 6.3 Mutation-check element attribution (§9). → verify: red, naming both collapsed
  ids. Confirmed on real data too — all 3255 XML-sourced edges have an `xml_element`
  source and none a document, and an element id reads
  `changeSet=<id>/modifyDataType~50`, so a table's references point at the operation
  that named it.
- [x] 6.4 Mutation-check the shared extractor: give the bridge its own SQL parse path.
  → verify: red across five tests, including the one asserting an attribute value and a
  statement land on one identity.
- [x] 6.5 Mutation-check the external mint. → verify: red across four tests, including
  the undeclared-table one. This is the mutation that matters most here: without minting,
  the 85 attribute-named tables — the entire gap 6.2 measures — vanish silently.
- [x] 6.6 `cargo clippy --workspace --all-targets` clean, `just crap-ci` green, then
  `cargo fmt --all`, then clippy once more (§7 ordering). Clippy flagged `validate`
  over the line limit once the rule check landed; split out `validate_globs` and
  `validate_xml_sql` rather than raising the ceiling.
