## ADDED Requirements

### Requirement: Embeddable text is doc-only and skips undocumented symbols

The embeddable text for a symbol SHALL be its documentation prose only, not the
signature (the `sig\ndoc` blend is retired). A symbol with no documentation SHALL
NOT be embedded (no `vec0` row). The embeddable-text fingerprint that drives
incremental re-embedding SHALL be derived from the doc text only, so a
signature-only source change does not force a re-embed. Search SHALL function
correctly for symbols without a vector, using the lexical arms alone. The
doc-only recipe SHALL NOT regress documented-symbol conceptual recall versus
`sig+doc` on any measured corpus (validated: Rust +19% in-fusion, TypeScript
tie, C# +2% on cleaned docs).

#### Scenario: documented symbol embeds its doc only

- **GIVEN** a symbol with a documentation comment
- **WHEN** the embedding pass runs
- **THEN** the vector is computed from the doc prose, not `sig\ndoc`
- **AND** a signature-only edit to that symbol does not change its embeddable
  fingerprint

#### Scenario: undocumented symbol is not embedded

- **GIVEN** a symbol with no documentation
- **WHEN** the embedding pass runs
- **THEN** no `vec0` row is written for it
- **AND** it remains findable through the lexical (identifier / name-token /
  signature) search arms
