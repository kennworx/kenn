## ADDED Requirements

### Requirement: Markdown is indexed as nodes in the unified graph

kenn SHALL index Markdown files as nodes and edges in the **same** store and
graph as code, such that a Markdown file and its sections are addressable,
searchable, and navigable through the existing read APIs. Markdown ingestion
SHALL emit `kenn_model` records directly (a sibling producer to the SCIP path);
it SHALL NOT route through the SCIP `transform` layer.

#### Scenario: A markdown file becomes searchable nodes

- **WHEN** an index run includes a `.md` file under an indexed markdown root
- **THEN** the file and each of its headings are present as nodes in the
  published snapshot
- **AND** `search_symbols` / `semantic_search` can return those nodes
- **AND** no SCIP document is produced for the markdown file

### Requirement: Two corpus modes — in-repo and external vault roots

The indexer SHALL support markdown roots both **inside** the indexed repository
and as **external vault roots** configured alongside it. Each root SHALL have a
stable label used in node identity. External roots reside outside the repository
working tree.

#### Scenario: In-repo markdown is indexed

- **WHEN** markdown files exist under the repository (e.g. `docs/`)
- **THEN** they are indexed under the `workspace` root label

#### Scenario: An external vault root is configured

- **WHEN** an external markdown directory is configured as a vault root with a
  label
- **THEN** its `.md` files are indexed under that label
- **AND** the file watcher treats changes under that root as index-affecting

### Requirement: Markdown discovery via search and exclude globs

Markdown roots SHALL be configured in a dedicated `MarkdownConfig` (its own field
on the per-language config) as **search globs** (include) plus **exclude globs**
over files and directories, holding raw pattern strings. A glob naming a
**directory** SHALL mean "index every `.md` beneath it, recursively"
(`<dir>/**/*.md`); a glob MAY name individual files. Exclude globs SHALL remove
matches from the discovered set. The markdown walker SHALL compile and apply
these globs itself at discovery time (markdown owns discovery and does not route
through `Workspace::is_excluded`). An external vault root MAY carry a label used
in node identity.

#### Scenario: A directory glob discovers markdown recursively

- **WHEN** a search glob names a directory containing `.md` files in nested
  subdirectories
- **THEN** every `.md` under that directory, at any depth, is discovered

#### Scenario: An exclude glob removes matches

- **WHEN** a discovered path matches a configured markdown exclude glob
- **THEN** that file is not indexed

### Requirement: Section granularity with modeled heading nesting

Each heading SHALL be a node whose definition span is its section (from the
heading line to the next heading of the same or higher level). The
`#`>`##`>`###` hierarchy SHALL be modeled with `contains` / `defined_in` edges
derived directly from heading levels, without positional enclosing heuristics.
Section prose SHALL be the unit fed to full-text and embedding indexes.

#### Scenario: Nested headings form a containment tree

- **WHEN** a file has `# A` containing `## B` containing `### C`
- **THEN** `list_in_scope` on the file node returns `# A`
- **AND** the parent (enclosing) of `### C` resolves to `## B`
- **AND** `find_at_location` on a line inside C's section returns `### C`

#### Scenario: Section prose is independently searchable

- **WHEN** distinct prose appears under two sibling sections
- **THEN** `semantic_search` can return the matching section node, not only the
  file

### Requirement: Frontmatter is parsed into metadata and drives resolution

YAML frontmatter SHALL be parsed during the collect phase and stored as metadata
on the file node (including `title`, `aliases`, `tags`). `title` and `aliases`
SHALL additionally populate the global resolution index so that links may
resolve to a file by its title or an alias.

#### Scenario: Alias resolves a wikilink

- **WHEN** a note declares `aliases: [foo-alias]` in frontmatter
- **AND** another note links `[[foo-alias]]`
- **THEN** the link resolves to the aliased note

#### Scenario: Frontmatter is queryable metadata

- **WHEN** a file declares `title` and `tags` in frontmatter
- **THEN** those values are stored on the file node as metadata

### Requirement: Two-phase build running parallel to code ingest

Markdown ingestion SHALL run as a **two-phase** build: phase 1 collects
frontmatter and heading slugs for every `.md` to build the global resolution
index; phase 2 full-parses bodies and emits nodes and edges. Markdown ingestion
SHALL run **concurrently** with code ingest units. Markdown-to-markdown links
SHALL be resolvable within phase 2 without waiting on code ingest.

#### Scenario: Collect precedes resolution

- **WHEN** note X links `[[Y]]` and note Y is defined later in directory order
- **THEN** the link from X resolves to Y, because Y was registered in the
  collect phase before any resolution

#### Scenario: md↔md graph does not wait on code

- **WHEN** a run indexes both code and a markdown vault with no code references
- **THEN** the markdown-to-markdown link graph is resolved without blocking on
  code ingest completion

### Requirement: Markdown node identity carries the corpus root

Markdown node public IDs SHALL take the form `md:<root-label>/<relpath>` for
files and `md:<root-label>/<relpath>#<heading-slug>` for sections. Heading slugs
SHALL be GitHub-style slugifications of heading text, de-duplicated within a file
(`-1`, `-2`, …). IDs SHALL be slug-based rather than line-based.

#### Scenario: Two roots with the same relative path stay distinct

- **WHEN** an in-repo file `notes/x.md` and a vault file `notes/x.md` both exist
- **THEN** they have distinct node IDs differentiated by root label

#### Scenario: Duplicate headings in one file get distinct slugs

- **WHEN** a file contains two `## Notes` headings
- **THEN** their section node IDs differ by a numeric suffix
