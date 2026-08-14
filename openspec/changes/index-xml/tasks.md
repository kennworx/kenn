## 1. Model

- [x] 1.1 Add `Language::Xml` (`crates/kenn-model/src/language.rs`) with
  `prefix`/`db_name`/`extensions` arms and the `from_prefix`/`from_db_name` round
  trips. Add it to the no-project-files arm alongside markdown/css/html/text — an XML
  file restructures nothing the way a manifest does. → verify: `from_db_name("xml")`
  resolves; every existing `match` on `Language` still compiles exhaustively.
- [x] 1.2 Add `Kind::XmlElement` (`crates/kenn-model/src/kind.rs`), following the
  `CssClass`/`HtmlId` precedent. Reuse `Kind::Document` for the file node, as markdown,
  HTML, and the text fallback already do. `XmlElement` is `false` for `is_scope`,
  `is_class_like`, and `is_callable` — it is not a nominal type and must stay out of
  nearest-enclosing aggregate-id rollup. → verify: `db_names_are_unique` passes; the
  kind round-trips `db_name`/`from_db_name`; it appears in no predicate set.
- [x] 1.3 Add `crates/kenn-model/src/id/xml.rs`: `document_id(relpath)` →
  `xml:<relpath>`, and `element_id(relpath, segments)` →
  `xml:<relpath>#<seg>/<seg>/…`. A segment is `<tag>=<value>` when the element carries
  `id`/`name`, else `<tag>~<ordinal>`. **Every** segment is discriminated, not just the
  last. Bracket forms (`<tag>[<ordinal>]`) are NOT available: a `pub_id` is handed to
  `kenn get` as a single shell token, and the writer's shell-safe assertion rejects
  brackets. Arbitrary attribute values are sanitised to the safe alphabet, `/` included,
  so a value cannot forge a segment separator. → verify: two `groupId` elements under
  different `dependency` parents get different ids (spec scenario); an element with an
  `id` attribute keeps its id
  when a sibling is inserted before it.

## 2. Config

- [x] 2.1 Add `XmlConfig` (`crates/kenn-config/src/language/xml.rs`) with an
  `extensions: Vec<String>` field defaulting to `["xml"]` and an `excludes` field
  defaulting to the build and vendor directories (`bin`, `obj`, `packages`,
  `node_modules`, `target`), wired into `LanguageConfig`, disabled by default,
  mirroring `CssConfig`'s shape — `rust.excludes` already defaults to `target/**`, so
  the pattern exists. The excludes are load-bearing, not hygiene: measured on a real
  repository, build and vendor directories held **10854** `.xml` files against **485**
  first-party ones, so an unexcluded walk indexes 22× more noise than content. →
  verify: round-trips through config load; disabled by default; the default claim is
  `.xml` alone; a file under a default-excluded directory is not walked.
- [x] 2.2 Register the claim from the configured extension list rather than a constant,
  so an added extension is walked and an absent one is not. → verify: the
  claimed-extension test asserts `xml` claimed when enabled, absent when disabled, and
  a configured extra extension claimed.

## 3. Parser

- [x] 3.1 Add `crates/kenn-indexer/src/xml/parse.rs` over `roxmltree` (read-only
  positioned DOM, one mandatory dep `memchr`): document text → a flat element list,
  each carrying its ancestor chain, `id`/`name` attribute when present, sibling
  ordinal, own text, attributes, resolved namespace, and byte range. Chosen over
  `quick-xml` because the containment edges need parent/child and the ranges are needed
  directly; config-sized files make streaming's memory advantage irrelevant. → verify:
  a fixture document yields one entry per element with a byte range that selects that
  element's source text.
- [x] 3.2 Attribute text to the element that directly contains it, not to an ancestor.
  → verify: a nested element holding the text owns it and its parent does not (spec
  scenario).
- [x] 3.3 Record the resolved namespace rather than the source prefix. → verify: the
  same document written with a default namespace and with an explicit prefix yields
  identical recorded namespaces.
- [x] 3.4 Return a positioned error on malformed input rather than panicking;
  `roxmltree` reports e.g. `expected 'b' tag, not 'a' at 1:7`. → verify: malformed
  input produces an error carrying a position, and no panic.

## 4. Producer — barrier-free unit

- [x] 4.1 Add `xml_phase1_unit` as a sibling ingest unit in
  `crates/kenn-indexer/src/pipeline/api.rs`, modelled on `text_unit` (barrier-free, no
  pending state). → verify: an XML-only workspace indexes with no barrier step running.
- [x] 4.2 Emit a `Document` node per file and an `XmlElement` node per element, with
  containment edges linking each element to its parent and the root to the document. →
  verify: a three-level document is walkable from the document to the innermost
  element and back up.
- [x] 4.3 Carry attributes and text on the element node; emit no attribute nodes. →
  verify: an element with several attributes yields exactly one node, and its
  attributes are readable from it.
- [x] 4.4 Replace `lexical_text` with a **signature renderer**: emit the element's start
  tag as well-formed markup (`<createTable tableName="users">`), escaping attribute
  values so the rendering round-trips. Rendering rather than slicing the source matches
  how code signatures are produced — `format_signature_documentation`
  (`crates/kenn-indexer/src/transform/document/walk.rs`) takes what the indexer
  rendered, it does not slice the file — and a canonical rendering is what makes a later
  consumer's attribute lookup exact instead of heuristic. → verify: an attribute whose
  value contains a space round-trips name and value out of the stored signature (spec
  scenario).
- [x] 4.5 Write the element's own text to the content surface (`SymbolDocsRecord::doc`)
  verbatim — no tag prefix, no attribute text. The prefix is not cosmetic: a stored
  `sql ALTER TABLE users …` is rejected by `sqlparser` at token 1, which is what makes
  the flattened form unusable to the bridge. → verify: an element whose text is a SQL
  statement stores that statement exactly and a parser accepts the stored value (spec
  scenario).
- [x] 4.6 Derive the lexical projection for XML from **both** surfaces — markup flattened
  to words plus the content text — in the existing XML arm of `build_name_rows`
  (`crates/kenn-store/src/db/sqlite/writer/finalize.rs`), which today passes the raw
  signature through instead of identifier-splitting it. Keep values verbatim: no
  identifier splitting, no `=` glue. This preserves the substring reach that moving text
  to the content surface would otherwise cost. SQL takes the same treatment (`index-sql`
  task 5b.1) — build **one** verbatim arm covering both languages, not two. → verify: a
  version pin in an attribute and a version pin in element text are both findable by
  substring search (spec scenarios).
- [x] 4.7 Exclude XML from the embedding selection in `scan_rows`
  (`crates/kenn-store/src/db/jobs.rs`), which selects on non-empty content with no
  language filter. The previous "leave the content surface unfed" strategy no longer
  applies now that element text lives there, and relying on it would silently enrol
  every text-bearing element in the embedding pass. SQL is excluded by the same filter
  (`index-sql` task 5b.2) — one filter, both languages. → verify: an XML-only workspace
  produces zero vectors after an embed pass (spec scenario); mutation-check by removing
  the filter and confirming vectors appear.
- [x] 4.8 Ensure every element's enclosing chain terminates at its document node, so
  elements roll up rather than becoming their own aggregates. `is_aggregate_leaf`
  (`crates/kenn-indexer/src/aggregate.rs`) accepts class-like kinds plus `Document` and
  `Attachment` — since `XmlElement` is none of those, an element walks up to its
  document and collapses there. Measured on a real repository: **30410** elements
  across **485** files roll up to **483** aggregates, a 63:1 collapse comparable to
  markdown's, which is why a numerically dominant document language does not distort
  the atlas. A broken enclosing chain silently turns each element into its own
  aggregate and that ratio to 1:1. → verify: an XML-heavy workspace's aggregate node
  count is within one of its file count, not its element count.
- [x] 4.9 Isolate per-file read and parse failures into the unit's `RunReport` and
  continue with the remaining files. → verify: one malformed file among several
  degrades the report, names the file and position, and leaves the others indexed
  (spec scenario).

## 5. Neutrality

- [x] 5.1 Keep every third-party vocabulary out of the implementation: no framework
  element name, attribute name, or namespace URI. The only known attribute names are
  `id` and `name`. → verify: a search of the XML indexer's source finds no third-party
  vocabulary term (spec scenario).
- [x] 5.2 Confirm on real documents that generic walking recovers their structure
  without recognizing them. → verify: measured on a private workspace holding 1370 real
  XML documents (the `tmp/xmlspike` evidence was hand-written). Changelog-style
  documents recover as
  `databaseChangeLog~0/changeSet=20260305_01_add_frozen_balance/comment~0` with correct
  ranges (changeSet 12-23, comment 13-13) and rendered signatures; a manifest's repeated
  `<include>` leaves get distinct ordinals `include~0 … include~12`. Nothing in the
  producer names a single vocabulary term to achieve it.

## 6. Verification

- [x] 6.1 A real repository containing XML indexes end to end. → verify: 1370 documents
  and 82799 elements, no failures beyond 2 unusable documents.

  **This found a defect the hand-written fixtures could not.** 17 `pub_id`s collided. A
  named segment assumed the identity attribute distinguishes siblings; two
  `<configuration name="app">` siblings differed only in a `type` attribute,
  and because a child's chain is built from its parent's, every descendant collided too —
  two elements produced 17 colliding ids. Reaching for `type` would mean knowing one
  vocabulary's business, so `Segment::named_at` disambiguates by position, and only when
  the value is actually shared: a unique name keeps its stable, position-free id.
  Re-measured after the fix: **0 collisions, element count unchanged**.
- [x] 6.2 Mutation-check the id-uniqueness guard (§9): discriminate only the final path
  segment, confirm the repeated-leaf test goes red for that reason, restore. Measured
  on a real repository, leaf-only discrimination collides on **69.4%** of elements
  (21108 of 30410) — this is the single highest-impact guard in the change, so the
  mutation must be seen to fail. → verify: red on mutation, green after.
- [x] 6.3 Mutation-check text attribution: attribute text to the ancestor instead of
  the containing element, confirm the nesting test goes red, restore. → verify: red on
  mutation, green after.
- [x] 6.4 `cargo clippy --workspace --all-targets` clean, `just crap-ci` green, then
  `cargo fmt --all`, then clippy once more (§7 ordering).
