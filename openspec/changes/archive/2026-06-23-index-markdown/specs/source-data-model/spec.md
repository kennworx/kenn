## ADDED Requirements

### Requirement: Markdown public IDs use the `md:` prefix with path/anchor native form

The public-ID scheme SHALL include an `md:` language prefix for markdown nodes.
The native-ID portion SHALL be path/anchor-based rather than symbol-native:
`md:<root-label>/<relpath>` for a markdown file and
`md:<root-label>/<relpath>#<heading-slug>` for a section. This extends the
existing `<lang>:<native-id>` scheme additively and SHALL NOT change the form of
existing code-language IDs.

#### Scenario: A markdown section has a path/anchor public ID

- **WHEN** a section `## Flow` exists in `docs/auth.md` under the `workspace`
  root
- **THEN** its public ID is `md:workspace/docs/auth.md#flow`

#### Scenario: Code IDs are unchanged

- **WHEN** markdown indexing is enabled
- **THEN** existing `cs:` / `ts:` / `rs:` / `go:` / `py:` IDs are unaffected

### Requirement: Edge-kind enum includes `links_to` and `embeds`

The edge-kind enum SHALL include `links_to` (a reference from one node to
another) and `embeds` (transclusion — the source node inlines the target's
content). These are additive; existing code edge kinds retain their meaning.

#### Scenario: A markdown reference and transclusion use the new kinds

- **WHEN** a markdown node references another node and transcludes a third
- **THEN** the first edge has kind `links_to` and the second has kind `embeds`

### Requirement: Markdown file and section node kinds

The kind enum SHALL include `document` (the markdown file-as-node) and `section`
(a heading). Both SHALL be represented as symbol-space nodes (so link edges
target them unambiguously), carry the `md` language value, and carry their `md:`
native ID as `pub_id`. A `FileRecord` with language `md` is also emitted for the
files table and change detection, but link edges SHALL target the `document` /
`section` symbols rather than the file record.

#### Scenario: A section node is a markdown-typed symbol

- **WHEN** a heading is indexed as a node
- **THEN** it is a symbol of kind `section`, language `md`, with `pub_id`
  `md:<root>/<relpath>#<slug>`

#### Scenario: A markdown file is a document symbol

- **WHEN** a markdown file is indexed
- **THEN** a symbol of kind `document` with `pub_id` `md:<root>/<relpath>` is
  emitted as the link-target node for the whole file
- **AND** a `FileRecord` with language `md` is also emitted for the files table
