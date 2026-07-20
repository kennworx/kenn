## ADDED Requirements

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

### Requirement: Wire location format is `./file_path#start-end`

Every API response field that carries a source location SHALL use the format `./<workspace_relative_path>#<start_line>` (single line) or `./<workspace_relative_path>#<start_line>-<end_line>` (line range).

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
