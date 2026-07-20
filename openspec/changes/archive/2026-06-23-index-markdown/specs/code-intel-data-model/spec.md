## ADDED Requirements

### Requirement: Edge kinds include `links_to` and `embeds`

The enumerated edge kinds SHALL include `links_to` (a reference from one node to
another) and `embeds` (transclusion — the source node inlines the target's
content), in addition to the existing code edge kinds. A `links_to` edge SHALL
be able to carry a match-quality grade (reusing the `match_kind` vocabulary) and
an optional relation. These additions SHALL NOT change the meaning of existing
code edge kinds.

#### Scenario: A graded markdown link is representable

- **WHEN** a markdown link resolves with a drifted match
- **THEN** a `links_to` edge is emitted carrying the drifted match-quality grade
- **AND** it round-trips through the store unchanged

#### Scenario: Transclusion uses the embeds kind

- **WHEN** a markdown node transcludes another via `![[…]]`
- **THEN** an `embeds` edge (distinct from `links_to`) is emitted

### Requirement: Markdown document and section identity

A markdown `document` and `section` symbol SHALL participate in the
`(canonical_path, symbol_string, range)` identity key using its `md:` native ID
as the `symbol_string` analog. The dedup/identity path SHALL accept a node whose
`symbol_string` is a markdown native ID rather than a code symbol string.

#### Scenario: Two sections in one file are distinct nodes

- **WHEN** a file has two headings producing native IDs `…#a` and `…#b`
- **THEN** they are retained as separate nodes keyed by their distinct
  `symbol_string` analogs at their respective ranges

#### Scenario: Document and its sections nest

- **WHEN** a file's `document` symbol contains heading sections
- **THEN** each section's `enclosing_sym_id` resolves to its parent section or
  the document symbol
