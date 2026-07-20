# Design — implement `NormalizeDocXml` (producer-side prose extraction)

**D1 — Clean at the producer (.NET sidecar), not on Rust ingest.** The raw XML
originates in `GetDocumentationCommentXml()`, and there is already a dedicated
`NormalizeDocXml` hook at the emission site — it is simply unimplemented. The
sidecar has the structured XML and `System.Xml.Linq`, so it can extract prose
*correctly* (entity decoding, nested tags) rather than a lossy Rust tag-strip.
Fixing it here also means no language-aware cleaning is needed in the Rust
`build_docs_record` path — C# is the only producer emitting this envelope.

**D2 — Parse, don't regex.** `GetDocumentationCommentXml()` returns well-formed
XML wrapped in a single `<member name="…">` root. Parse with `XDocument`/
`XElement` and walk it: concatenate the text of the prose-bearing elements
(`summary`, `remarks`, `returns`, `value`, `example`, and each `param`/`typeparam`
body), in document order, separated by blank lines. Inline reference tags
(`see cref`, `seealso`, `paramref name`, `typeparamref name`) contribute their
bare symbol/parameter name. `<c>`/`<code>` contribute their inner text. Unknown
tags contribute their text content (never their tag name). A tag-strip regex
would mishandle entities and `cref` attributes; the parser is both simpler and
correct.

**D3 — `<inheritdoc/>` → empty.** A doc whose only content is `<inheritdoc/>`
has no inline prose; it normalizes to empty (and `NormalizeDocXml` returns
`null`, same as "no doc"). Resolving the inherited doc across base
types/interfaces is a much larger feature and is out of scope. Result: such
symbols are honestly treated as undocumented for ranking/embedding.

**D4 — Whitespace + robustness.** Collapse the pretty-printed indentation to
single spaces and trim; join sections with a single blank line. If parsing fails
for any reason (malformed XML, unexpected shape), fall back to returning `null`
rather than the raw XML — never leak markup downstream. Empty result → `null`.

**D5 — `param`/`returns` rendering.** Keep them as readable prose, e.g.
`"<param name=\"id\">the order id</param>"` → `"id: the order id"` (or just the
body text). Exact phrasing is a detail for implementation; the requirement is
"human text, no tags, no FQN envelope."

## Out of scope

- Inherited-doc (`<inheritdoc/>`) resolution.
- TS JSDoc / Rust markdown normalization (already prose).
- Migrating existing snapshots — takes effect on reindex.
