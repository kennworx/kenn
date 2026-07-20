## MODIFIED Requirements

### Requirement: Documentation Inline on Symbol Frames

The producer SHALL inline `signature_doc` and `documentation` strings on
the `symbol` frame itself when present, omitting them when absent. The
`documentation` string SHALL be **plain prose** — the producer SHALL normalize
the XML doc comment returned by Roslyn into human-readable text, stripping the
`<member>` envelope and all doc tags, keeping the text of prose elements
(`summary`, `remarks`, `returns`, `value`, `example`, `param`, `typeparam`),
rendering inline reference tags (`see cref`, `paramref`, …) as their bare names,
and decoding XML entities. A doc whose only content is `<inheritdoc/>` (no
inline prose) SHALL be treated as absent (no `documentation` string emitted).
The consumer SHALL split these out into a separate `SymbolDocsRecord` row only
for symbols where at least one of the two strings is non-empty.

#### Scenario: Symbol with docs gets one symbol_docs row of plain prose

- **WHEN** a class has an XML doc comment `<summary>Holds the order.</summary>`
- **THEN** its `symbol` frame contains `documentation` equal to the prose
  `Holds the order.` (no `<member>`, `<summary>`, or `name="…"` markup)
- **AND** the consumer writes one `SymbolDocsRecord` row for it

#### Scenario: Symbol without docs gets no symbol_docs row

- **WHEN** a class has no signature renderer output and no XML docs
- **THEN** neither `signature_doc` nor `documentation` appears on its
  `symbol` frame
- **AND** the consumer writes no `SymbolDocsRecord` row for it

#### Scenario: inheritdoc-only comment emits no documentation

- **WHEN** a member's only doc comment is `<inheritdoc/>`
- **THEN** no `documentation` string is emitted for it (it is treated as
  undocumented; the inherited doc is not resolved)
