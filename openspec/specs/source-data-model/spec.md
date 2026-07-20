# source-data-model

## Purpose

Defines the normalized logical source-data-model that all producers write into and all readers (MCP server, web UI, CLI inspector, future LSP bridges) consume. It pins down the public symbol-ID format (per-language native syntax with a short language prefix), internal short-ID strategy (u32 cross-references in the DB, public IDs only at the API boundary), table and graph-relation shapes, kind enum, and the `./file_path#start-end` wire location format. Multi-language by default — a single uniform schema with a `language` column rather than per-language tables.
## Requirements
### Requirement: Public symbol IDs use language prefix + native syntax

Every symbol exposed through any read API SHALL have a public ID of the form `<lang>:<native-id>` where `<lang>` is the two-letter language prefix (`cs`, `ts`, `rs`, `go`, `py`) and `<native-id>` is the language-native semantic location.

The native ID portion SHALL follow the conventions a developer in that language community uses to refer to the symbol:
- C#: `Namespace.Type.Member(paramTypes)` with parameter list for overloads
- TypeScript: `<package>/<file-without-ext>.Symbol`
- Rust: `crate::path::to::item`
- Go: `package_path.Symbol` or `package_path.Type.Method`
- Python: `module.Class.method`

Verbatim SCIP symbol strings SHALL NOT be exposed publicly.

#### Scenario: Cross-language ID round-trip from SCIP

- **WHEN** a SCIP symbol from any supported language is processed
- **THEN** the system MUST produce a public ID with the correct language prefix
- **AND** parsing the public ID back MUST recover the language and the native ID portion

#### Scenario: Public IDs do not contain indexer prefixes

- **WHEN** any public ID is materialized
- **THEN** the ID MUST NOT contain `scip-typescript`, `scip-dotnet`, `rust-analyzer`, `gomod`, `nuget`, or any other indexer-tooling string
- **AND** the ID MUST NOT contain package version numbers

### Requirement: Public IDs are stable across non-semantic edits

A symbol's public ID SHALL remain unchanged across:
- File renames (except in TypeScript, where the file IS the module)
- Code moves within the same file
- Code moves between files belonging to the same module (where languages support multi-file modules: Rust, Go, C#, Python packages)

A symbol's public ID SHALL change when:
- The symbol is renamed
- The symbol is moved to a different module/namespace/package
- For TypeScript only: the containing file is renamed

#### Scenario: File-rename stability

- **WHEN** a C#/Rust/Go/Python file is renamed without changing the namespace/module of any symbol it contains
- **THEN** every symbol's public ID after reindex MUST equal its public ID before the rename

#### Scenario: Symbol-rename produces a new ID

- **WHEN** a method is renamed (semantic change)
- **THEN** the post-reindex DB MUST NOT contain a symbol with the old public ID
- **AND** queries against the old public ID MUST return a not-found response

### Requirement: Internal cross-references use u32 short IDs

Within the persisted database, every cross-reference between rows SHALL use a `u32` integer (`short_id` for symbols, `id` for files) — never a string public ID. Public IDs SHALL exist only on the `symbols.id` column and SHALL be translated to/from short IDs at the API boundary.

The reserved value `0` SHALL serve as a "no reference" sentinel for fields like `enclosing_symbol`. Auto-increment IDs MUST start at `1`.

#### Scenario: Short-id translation is bijective for non-zero values

- **WHEN** a short_id is translated to a public ID and back via the symbols table
- **THEN** the result MUST equal the original short_id
- **AND** the reserved short_id `0` MUST never appear as a primary key

### Requirement: No nullable columns

The schema SHALL NOT use nullable columns. Every column SHALL have a default value:
- Integer foreign keys default to `0` (sentinel for absent reference)
- `u8` arity columns default to `0`
- Booleans default to `false`
- Strings default to empty

Queries that need to disambiguate "0 means absent" from "0 is the actual value" SHALL filter on the `kind` column or use composite checks.

#### Scenario: Inserting a symbol without specifying optional fields uses defaults

- **WHEN** a symbol record is inserted with only required identity fields
- **THEN** all other columns MUST receive their declared default values
- **AND** no NULL value MUST appear anywhere in the row

### Requirement: files table is the source of truth for file metadata

A `files` table SHALL exist with columns: `short_id u32 PK auto-increment`, `path string`, `language string`, `is_test bool`, `is_external bool`, `content_hash u64`. The `path` SHALL be workspace-relative and canonical. (The column is named `short_id` rather than `id` because SurrealDB reserves `id` for the record-identifier primary key; the auto-increment u32 is stored as `short_id`.)

Every symbol or relation that refers to a file SHALL reference `files.short_id`, not the path string.

`content_hash` SHALL be xxhash64 of the file's contents at ingest time.

#### Scenario: A symbol references a file by id, not path

- **WHEN** a symbol record is fetched
- **THEN** the `file` field MUST be a `u32` referring to a row in the `files` table
- **AND** the file's path MUST be retrievable via a single lookup against `files.short_id`

#### Scenario: content_hash is set on every ingested file

- **WHEN** ingest completes for a workspace
- **THEN** every row in `files` MUST have a non-zero `content_hash`
- **AND** the hash MUST equal `xxhash64` of the file's bytes at ingest time

### Requirement: symbols table holds the primary def location and metadata

A `symbols` table SHALL exist with columns:
- `short_id u32 PK auto-increment`
- `pub_id string` (the public ID; named `pub_id` rather than `id` because SurrealDB reserves the `id` column for the record-identifier primary key)
- `language string`
- `kind` (closed enum, see kind enum requirement)
- `name string`
- `display_name string`
- `enclosing_symbol u32 default 0`
- `file u32`
- `def_range [u32; 4]`
- `is_partial bool default false`
- `args_arity u8 default 0`
- `generic_arity u8 default 0`
- `is_external bool default false`
- `is_test bool default false`

Indexes: `(language, pub_id)` UNIQUE; `name` FULLTEXT (BM25; analyzers filter by `language` post-search since SurrealDB FULLTEXT indexes target a single field); `(language, kind)`; `file`; `enclosing_symbol`.

The `enclosing_symbol` field SHALL reference the symbol's direct parent (any kind) — not the nearest module/namespace ancestor. For top-level symbols whose parent is the workspace root, `enclosing_symbol = 0`.

#### Scenario: Direct-parent lookup is one PK fetch

- **WHEN** a symbol's enclosing parent is requested
- **THEN** the parent MUST be retrievable as `SELECT * FROM symbols WHERE short_id = <child>.enclosing_symbol`
- **AND** when `enclosing_symbol = 0`, the response MUST indicate the symbol is top-level

### Requirement: Kind enum is closed and language-agnostic

The `kind` enum SHALL be a closed set: `package`, `module`, `namespace`, `class`, `struct`, `interface`, `trait`, `enum`, `enum_member`, `type_alias`, `method`, `function`, `constructor`, `destructor`, `operator`, `field`, `property`, `constant`, `variable`, `parameter`, `type_parameter`, `macro`.

The producer SHALL map indexer-emitted kinds (when present) and SCIP descriptor suffixes (when not) to this set deterministically.

#### Scenario: Indexer-emitted kind maps to this enum

- **WHEN** scip-go or rust-analyzer emits `SymbolInformation.kind`
- **THEN** the producer MUST translate it to a value from this enum via a per-indexer mapping table

#### Scenario: SCIP descriptor suffix derives kind when indexer is silent

- **WHEN** scip-dotnet, scip-typescript, or scip-python emit `SymbolInformation.kind = 0`
- **THEN** the descriptor suffix grammar (`#`, `().`, `.`, `(name)`, `[T]`, trailing `/`) MUST be parsed and mapped to a kind from this enum

### Requirement: defined_in relation expresses semantic location

A `defined_in` graph relation SHALL exist with source `symbols` and target `symbols` where the target's `kind` is `package`, `module`, or `namespace`. Each non-top-level symbol SHALL have exactly one `defined_in` edge to its most specific module/namespace ancestor.

For nested modules: a child module's `defined_in` MUST point to its parent module. For top-level modules in a package: `defined_in` MUST point to the package.

The `defined_in` relation MUST support efficient bidirectional traversal: "the module of X" and "all symbols in module M" SHALL both be queryable in single-digit milliseconds at production scale.

#### Scenario: Subtree query returns all transitive members

- **WHEN** the recursive backward traversal `<-defined_in<-..*` is executed from a module M
- **THEN** the result MUST contain every symbol whose enclosing module/namespace chain leads to M, at any depth

### Requirement: contains relation expresses physical layout (M:N)

A `contains` graph relation SHALL exist with source `symbols` (where source's `kind` is `package`, `module`, or `namespace`) and target `files`. The relation SHALL be many-to-many: a module MAY span multiple files, and a file MAY contain multiple modules.

#### Scenario: Multi-file module is reachable from any of its files

- **WHEN** a Rust module is declared with sources in `foo.rs` and `foo/bar.rs` belonging to the same `mod foo`
- **THEN** the `contains` relation MUST contain edges from the `mod foo` symbol to both files
- **AND** querying `<-contains<-* FROM files:foo_bar_rs` MUST include the `mod foo` symbol

### Requirement: Symbol-level relations are deduplicated at pair granularity

The relations `calls`, `type_use`, `field_access`, `implements`, `overrides`, `instantiates`, `generic_constraint` SHALL each enforce uniqueness on `(source, target)`. If a caller invokes a callee at multiple call sites, exactly one `calls` edge SHALL exist between them.

The persisted DB SHALL NOT carry per-call-site occurrence rows. Site-level information is recovered by reading the caller's source — outside the scope of this data model.

#### Scenario: Multiple call sites produce one calls edge

- **WHEN** MethodA invokes MethodB at three different lines within MethodA's body
- **THEN** the `calls` relation MUST contain exactly one row for `(MethodA, MethodB)`

### Requirement: corresponds_to relation expresses cross-boundary equivalence

A `corresponds_to` graph relation SHALL exist with `symbols`-to-`symbols` shape and properties:
- `source: enum { config, auto_inferred, codegen }`
- `generator: string` (e.g., `protoc`, `openapi`; empty when not codegen)
- `canonical: u32 default 0` (short_id of the source-of-truth symbol if any)

V1 SHALL support `source = config` only. Codegen-detection and auto-inference are out of scope.

#### Scenario: Config-declared correspondence is queryable

- **WHEN** the user declares two symbols equivalent in `kenn.toml`
- **THEN** a `corresponds_to` edge MUST exist between them with `source = config`
- **AND** the edge MUST be traversable in both directions

### Requirement: imports relation expresses module-to-module dependencies

An `imports` graph relation SHALL exist with `symbols`-to-`symbols` shape where both endpoints have `kind` ∈ {`package`, `module`, `namespace`}. Each (importing module, imported module) pair SHALL appear as exactly one edge.

The relation SHALL carry a `kind` property (`explicit` | `re_export`) distinguishing direct imports from re-exports (e.g., Rust `pub use`, TypeScript `export * from`).

The relation SHALL NOT exist at file granularity; agents that need the actual import statement positions read the importing module's files.

#### Scenario: Re-export is distinguishable

- **WHEN** module A re-exports symbols from module B (`export * from`, `pub use`)
- **THEN** the `imports` relation MUST contain an edge `A → B` with `kind = re_export`

### Requirement: def_range line basing is 1-based, column basing is 0-based

The `defs` table's `start_line` and `end_line` columns SHALL hold **1-based** line numbers (the editor convention: the first line of a file is line 1). The `start_col` and `end_col` columns SHALL hold **0-based** column numbers (the column of the first character on a line is column 0).

Ingest is the single boundary where this conversion happens. Producer wire formats — SCIP `Occurrence.range` and the dotnet `def_range` JSONL field — are 0-based on both axes (their native conventions). Each ingest transform SHALL add `+1` to `start_line` and `end_line` before pushing a `DefRecord`. Columns SHALL be stored as received.

Downstream consumers (MCP wire renderers, `find_at_location`, `get_source`) SHALL consume stored values directly with no further basing adjustment. Tool *inputs* that name a source line — today this is `find_at_location.line` — SHALL also be **1-based** so that values pasted from stack traces, compiler errors, editor "go to line", and prior MCP responses (`get_source.start_line`, wire `#<line>` format) round-trip without translation. The MCP tool description SHALL document this explicitly.

#### Scenario: A symbol declared on file line 16 stores start_line = 16

- **WHEN** a SCIP `Occurrence` for a definition reports `range = [15, 4, 15, 18]` (0-based)
- **THEN** the resulting `DefRecord` in the store MUST have `start_line = 16, start_col = 4, end_line = 16, end_col = 18` (lines `+1`, columns unchanged)
- **AND** `get_source` rendering that symbol MUST return the text of line 16 of the file

#### Scenario: dotnet JSONL frame with 0-based range stores 1-based lines

- **WHEN** a C# `symbol` frame arrives with `def_range = [9, 13, 9, 16]` (0-based, per `dotnet-stream-indexer`)
- **THEN** the resulting `DefRecord` MUST have `start_line = 10, start_col = 13, end_line = 10, end_col = 16`

#### Scenario: find_at_location accepts a 1-based line

- **WHEN** a function's declaration occupies file line 1868 and the agent calls `find_at_location(file_path, line=1868)`
- **THEN** the response MUST include that function as the smallest enclosing symbol
- **AND** a call with `line=1867` (the blank line above) MUST NOT match the function

#### Scenario: Synthetic / external symbols keep zero range

- **WHEN** a symbol is synthetic (no source location) or external (no in-workspace definition)
- **THEN** the `DefRecord` MUST be `[0, 0, 0, 0]` and the symbol MUST be marked `is_external = true`
- **AND** the wire location for the symbol MUST be `null`

### Requirement: Wire location format is `./file_path#start-end`

Every API response field that carries a source location SHALL use the format `./<workspace_relative_path>#<start_line>` (single line) or `./<workspace_relative_path>#<start_line>-<end_line>` (line range). Line numbers in this format are **1-based** — they match the stored `def_range` values, which match what an editor displays.

When no def location applies (synthetic symbols, external symbols without source), the location SHALL be `null`.

The format SHALL include only line numbers; column data SHALL NOT appear in the wire format. Column data remains in the DB on `def_range` for any consumer that needs it.

#### Scenario: Single-line and multi-line locations format correctly

- **WHEN** a class with `def_range = [3, 0, 14, 1]` is materialized
- **THEN** the wire location MUST be `./<path>#3-14`

- **WHEN** a single-line def with `def_range = [42, 8, 42, 32]` is materialized
- **THEN** the wire location MUST be `./<path>#42`

#### Scenario: External symbols have null location

- **WHEN** a symbol with `is_external = true` and `file = 0` is materialized
- **THEN** the wire location MUST be `null`

#### Scenario: First line of a file renders as #1, never #0

- **WHEN** a top-of-file symbol has `def_range = [1, 0, 1, N]`
- **THEN** the wire location MUST be `./<path>#1`
- **AND** the wire location MUST NOT be `./<path>#0`

### Requirement: Occurrences are not persisted in the live DB

The live database SHALL NOT contain an occurrences table. Occurrences MAY be materialized in memory (or in a debug-only rocksdb staging DB) during ingest as the substrate for relation derivation, but SHALL be discarded after the live DB is published.

A debug mode (e.g., `kenn index --debug-staging`) MAY persist occurrences for inspection; the default mode SHALL NOT.

#### Scenario: Live DB has no occurrences table

- **WHEN** the live DB is queried for table names
- **THEN** the result MUST NOT include `occurrences` (or any equivalent named table)

### Requirement: Test-file marking via glob configuration

A `kenn.toml` `[tests]` section SHALL accept a `paths` array of glob patterns. At ingest, files matching any glob SHALL have `files.is_test = true` and the flag SHALL be denormalized onto every symbol defined in those files (`symbols.is_test = true`).

#### Scenario: Glob match flags file and symbols

- **WHEN** `paths = ["**/*Test.cs"]` is configured and a file `Models/OrderTest.cs` is ingested
- **THEN** `files.is_test` for that file MUST be `true`
- **AND** every symbol with `file` referencing that file MUST have `is_test = true`

#### Scenario: No glob match leaves test flag false

- **WHEN** a file's path does not match any configured glob
- **THEN** `files.is_test` MUST be `false`
- **AND** symbols defined in it MUST have `is_test = false`

### Requirement: Multi-language schema with no per-language tables

The schema SHALL be uniform across languages. There SHALL NOT be per-language tables (no `symbols_csharp`, `symbols_typescript`, etc.). The `language` column on relevant tables SHALL identify the language, and queries SHALL filter or partition by it as needed.

Cross-language queries (e.g., "find all symbols matching name X across all languages") SHALL be expressible without UNION across multiple tables.

#### Scenario: Cross-language search returns mixed-language results

- **WHEN** a name search runs without a language filter
- **THEN** the result MAY include symbols from multiple languages
- **AND** each result row MUST carry its `language` field for downstream filtering

### Requirement: The graph models an `extends_type` augmentation edge

The data model SHALL define an `extends_type` edge kind whose source is a symbol
that augments a type from outside the type's own declaration (e.g. a C# extension
method) and whose target is the type being augmented. The edge is
**non-containment**: it SHALL NOT replace or duplicate the source symbol's
`defined_in` edge, which continues to point at the symbol's real declaring scope.
A type's augmenting symbols are its **incoming** `extends_type` edges. The kind
parallels the existing `extends_rule` (stylesheet `@extend`) — a non-containment
"extends" relation — and SHALL serialize as the string `extends_type` identically
on the JSONL wire and in the model.

#### Scenario: an extension method gains an edge to the type it extends

- **WHEN** a C# extension method `Foo` declared in `OrderExtensions` extends
  `Order`
- **THEN** the graph contains an `extends_type` edge from `Foo` to `Order`
- **AND** `Foo` retains its `defined_in` edge to `OrderExtensions`

#### Scenario: the augmented type lists its augmenting symbols

- **WHEN** `Order` is the receiver of two extension methods
- **THEN** `Order` has exactly two incoming `extends_type` edges, one per method

#### Scenario: the edge string is stable across wire and model

- **WHEN** an `extends_type` edge is serialized on the JSONL wire and parsed into
  the model
- **THEN** both spell the kind `extends_type`

### Requirement: Languages are identified by a stable two-character prefix and extension set

The data model SHALL recognize Swift as an indexed language with the prefix `sw`,
the source extension `.swift`, and the project file `Package.swift`. Swift public
IDs SHALL take the form `sw:<key>`, where `<key>` is the language-naked descriptor
emitted by the Swift sidecar and assembled by the consumer from
`MetaFrame.language`. Swift symbols SHALL use the existing node and edge kinds — no
Swift-specific kind is added: `protocol` maps to `interface`, `actor` to `class`,
`subscript` to `property`/`method`, and a Swift `extension`'s members are modeled
as members of the extended type (reusing the partial-declaration collapse), not as
a new kind or edge.

#### Scenario: a Swift symbol's public ID carries the sw prefix

- **WHEN** the Swift sidecar emits a symbol with key `Order#save()`
- **THEN** its public ID is `sw:Order#save()`

#### Scenario: the swift extension and project file are recognized

- **WHEN** the workspace contains `Sources/App/Order.swift` and `Package.swift`
- **THEN** `.swift` is treated as a Swift source extension and `Package.swift` as a
  Swift project file that triggers reindex on change

#### Scenario: a Swift protocol uses the interface kind

- **WHEN** a Swift `protocol Persistable {}` is indexed
- **THEN** it is modeled with the `interface` kind (no new `protocol` kind)

### Requirement: HTML is a modelled language with document and html_id nodes

The public-ID scheme SHALL gain an `html:` language prefix covering `.html` and
`.htm` files. Each HTML file SHALL be modelled as a `document` node (mirroring
markdown documents), and each `id="…"` attribute SHALL define an `html_id` node
with a typed native-ID form `html:<relpath>#id:<name>` (the `#id:` type segment
keeps it distinct from any other node namespace for the same file). HTML SHALL
reuse existing edge kinds — `LinksTo`, `LinksToFile`, `Imports`, `UsesCssClass`,
and `CorrespondsTo` — and SHALL NOT introduce a new edge kind. The edge for a
reference is chosen by its target's table: an indexed-file target uses
`LinksToFile`/`Imports`; a node or attachment-stub target uses `LinksTo`
(asset stubs included — HTML does not use the transclusion edge `Embeds`). The
HTML-id ↔ CSS-id relation reuses `CorrespondsTo`, not a usage edge.

#### Scenario: an html file gets the html prefix and a document node

- **WHEN** `pages/index.html` is indexed
- **THEN** it is modelled as a `document` node under the `html:` prefix

#### Scenario: an id attribute gets a typed html_id native id

- **WHEN** `pages/index.html` contains `<div id="root">`
- **THEN** an `html_id` node with native id `html:pages/index.html#id:root` exists

#### Scenario: no new edge kind is introduced for HTML

- **WHEN** HTML links, imports, asset refs, class usage, and id correspondence are emitted
- **THEN** they use only the existing `LinksTo`/`LinksToFile`/`Imports`/
  `UsesCssClass`/`CorrespondsTo` edge kinds

#### Scenario: an inline-style css node is owned by the HTML file

- **WHEN** `page.html` defines `.hero` in an inline `<style>` block
- **THEN** the node reuses `Kind::CssClass` with native id `css:page.html#class:hero`
  (the `css:` prefix marks it a CSS node, the HTML relpath records the owner; the
  kind stays shared, and it stays distinct from the `html_id` `#id:` namespace)

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

### Requirement: Stylesheet public IDs use `css:`/`sass:` prefixes with path/selector native form

The public-ID scheme SHALL include two stylesheet language prefixes: `css:` for
`.css` files and `sass:` for Sass files (`.scss` and `.sass` — one language with
two syntaxes). The native-ID portion SHALL be `<lang>:<relpath>#<type>:<name>`,
where `<type>` is one of `class`, `id`, or `var` and `<name>` is the atomic token
(class `.btn` → `class:btn`, id `#app` → `id:app`, custom property `--brand` →
`var:--brand`). The `<type>` segment is REQUIRED: a class and an id of the same
name in the same file (`.hero` and `#hero`) MUST produce distinct IDs. The
stylesheet file itself is a node with ID `<lang>:<relpath>`. This extends the
existing `<lang>:<native-id>` scheme additively and SHALL NOT change existing
code-language IDs. The `css`/`sass` split SHALL be source-provenance only: both
languages use the same stylesheet node kinds and feed one unified class registry.

#### Scenario: A CSS class has a typed `css:` public ID

- **WHEN** `.btn-primary` is defined in `src/button.css`
- **THEN** its public ID is `css:src/button.css#class:btn-primary`

#### Scenario: A same-named class and id do not collide

- **WHEN** a file defines both `.hero` and `#hero`
- **THEN** the class ID is `…#class:hero` and the id ID is `…#id:hero`
- **AND** the two nodes are distinct

#### Scenario: A Sass class has a `sass:` public ID

- **WHEN** `.btn-primary` is defined in `src/button.scss`
- **THEN** its public ID is `sass:src/button.scss#class:btn-primary`
- **AND** its node kind is `css_class` (kinds are shared across css/sass)

#### Scenario: Code IDs are unchanged

- **WHEN** stylesheet indexing is enabled
- **THEN** existing `cs:` / `ts:` / `rs:` / `go:` / `py:` / `md:` IDs are
  unaffected

### Requirement: Stylesheet node kinds

The kind enum SHALL include `css_class`, `css_id`, and `css_var`. Each is a
symbol-space node (so usage and dependency edges target it unambiguously), carries
the language value of its source file (`css` or `sass`), and carries its native
ID as `pub_id`. Each stylesheet **file** SHALL additionally be a scope node of
kind `module` (a stylesheet is a module of style rules): its selectors are
`defined_in` it and it `contains` them, and it is the valid endpoint for
`imports` edges (below). A `FileRecord` with the matching language (`css` or
`sass`) is also emitted for the files table and change detection.

#### Scenario: A class is a `css_class` node regardless of source language

- **WHEN** a class is indexed from a `.css` file and another from a `.scss` file
- **THEN** both nodes have kind `css_class`
- **AND** their languages are `css` and `sass` respectively

#### Scenario: A stylesheet file is a module node owning its selectors

- **WHEN** `src/button.css` defines `.btn`
- **THEN** a `module` node `css:src/button.css` exists
- **AND** `.btn` is `defined_in` it (and it `contains` `.btn`)

### Requirement: Edge-kind enum includes `uses_css_class` and `extends_rule`

The edge-kind enum SHALL include `uses_css_class` (a code file/symbol references a
CSS class) and `extends_rule` (`@extend .class` / `composes … from`, rule →
class). In v1 `extends_rule` targets SHALL be existing `css_class` nodes;
`@extend %placeholder` and `@include`/mixin (which would require placeholder/mixin
node kinds) are out of scope. `@import`/`@use`/`@forward` SHALL reuse the existing
`imports` edge kind, whose endpoints are the stylesheet **module** nodes (so the
reuse is consistent with `imports` being a module-to-module relation). These
additions are additive; existing edge kinds retain their meaning.

#### Scenario: A class usage and an @extend use the new kinds

- **WHEN** a component uses a class and a rule `@extend`s a placeholder
- **THEN** the first edge has kind `uses_css_class` and the second `extends_rule`

#### Scenario: An @use is a module-to-module imports edge

- **WHEN** `a.scss` contains `@use './b'`
- **THEN** an `imports` edge connects module `sass:a.scss` → module `sass:b.scss`

### Requirement: defs carry an enclosing-item body extent distinct from the name span

The `defs` table SHALL carry two additional columns, `body_start_line` and
`body_end_line`, holding the **1-based** line span of the whole enclosing item
(a function/method/type/impl body, including its outer doc comment and
attributes) that the definition names. These are **lines only** — no columns —
because the sole consumer, `get_source`, slices whole lines.

The body extent is distinct from the name span
(`start_line/start_col/end_line/end_col`), which continues to hold the
identifier range used by `find_at_location`, edge anchoring, and location
rendering. The body extent MUST NOT be derived by overloading the name span's
`end_line/end_col`.

A definition with no producer-supplied extent — an older rust-analyzer that
emits no `enclosing_range`, a synthetic/external symbol, or a producer that does
not yet emit a body range — SHALL store `body_start_line = 0` and
`body_end_line = 0` (the "absent" sentinel). The columns default to `0`.

Because the extent excludes trivia other than doc comments, `body_start_line`
MAY be **less than** `start_line` (the doc comment / attribute sits above the
name line).

#### Scenario: A multi-line function stores its whole-item span

- **WHEN** a definition's name is on file line 46 and its enclosing item spans
  lines 42–237 (a leading `#[…]` attribute through the closing brace)
- **THEN** the stored `DefRecord` MUST have `start_line = 46` (the name)
- **AND** `body_start_line = 42`, `body_end_line = 237`

#### Scenario: A definition with no producer extent stores zero body span

- **WHEN** an indexer supplies a definition with a name range but no enclosing /
  body range
- **THEN** the stored `DefRecord` MUST have `body_start_line = 0` and
  `body_end_line = 0`

### Requirement: get_source returns the enclosing item when an extent is present

`get_source` SHALL slice the stored body extent when it is present — defined as
`body_start_line >= 1` and `body_end_line >= body_start_line` — returning the
whole item (doc comment / attributes through the closing brace) and reporting
`start_line`/`end_line` equal to the body span it sliced.

When the body extent is absent (`body_start_line = 0`), `get_source` SHALL fall
back to the **name span** (`start_line … end_line`) — the declaration line for a
def whose name range is a single line. `get_source` SHALL NOT parse source to
synthesize an extent.

#### Scenario: full item returned when the extent is stored

- **WHEN** `get_source` is called for a symbol whose def has
  `body_start_line = 42, body_end_line = 237`
- **THEN** the response `start_line` MUST be 42 and `end_line` MUST be 237
- **AND** `text` MUST be lines 42–237 of the file

#### Scenario: declaration line returned when no extent is stored

- **WHEN** `get_source` is called for a symbol whose def has
  `body_start_line = 0` and a single-line name span at line 46
- **THEN** the response MUST return line 46 (the declaration line), unchanged
  from the pre-extent behavior

