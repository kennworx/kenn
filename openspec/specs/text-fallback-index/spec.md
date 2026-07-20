# text-fallback-index Specification

## Purpose
TBD - created by archiving change text-fallback-index. Update Purpose after archive.
## Requirements
### Requirement: Configured text files are indexed as chunked nodes

kenn SHALL provide a generic text-fallback producer that makes user-selected text
files searchable when no semantic or native producer handles them. It SHALL be
configured by include/exclude globs in its own config block and SHALL be
**disabled by default**. For each included file it SHALL emit a file node plus one
node per chunk directly as `kenn_model` records (a sibling producer, not routed
through SCIP), with each chunk's text fed to the FTS and embedding indexes. The
splitter SHALL be a size-bounded recursive character/line splitter (no
tree-sitter/AST dependency).

#### Scenario: a configured text file becomes searchable nodes

- **WHEN** an index run includes a `.yaml`/`.json`/`.txt` file matched by a
  fallback include glob, with the fallback enabled
- **THEN** the file and its chunks are present as nodes in the published snapshot
- **AND** `semantic_search` / `search_symbols` can return those chunk nodes

#### Scenario: the fallback is off by default

- **WHEN** no fallback config is set
- **THEN** no non-semantic text files are indexed and behavior is unchanged

#### Scenario: a small file is a single chunk

- **WHEN** an included file is below the minimum chunk size
- **THEN** it produces one chunk node covering the whole file rather than a split

### Requirement: The fallback never double-indexes a file another producer claims

The text-fallback producer SHALL skip any file whose extension is handled by an
enabled semantic or native producer, so a file is indexed by exactly one producer.

#### Scenario: an enabled producer's file is not also fallback-indexed

- **GIVEN** `[language.rust]` is enabled and a fallback glob would also match `.rs`
- **WHEN** an index run executes
- **THEN** `.rs` files are indexed only via the Rust producer, not chunked again by
  the fallback

