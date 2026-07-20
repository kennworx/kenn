# Tasks

## 1. Public ID format

- [x] 1.1 Define the `Language` enum (`Csharp`, `TypeScript`, `Rust`, `Go`, `Python`) with the lang-prefix string for each (`cs`, `ts`, `rs`, `go`, `py`)
- [x] 1.2 Define a per-language `IdTransformer` trait: `scip_to_public(scip_symbol: &str) -> Result<PublicId>` and `parse_public(id: &str) -> Result<ParsedId>`
- [x] 1.3 Implement `IdTransformer` for C#: strip scip-dotnet's `nuget . . ` prefix; recover `Namespace.Type.Member(paramTypes)`; recover overload signature from SCIP descriptor parameter list; encode generic arity only when needed for disambiguation
- [x] 1.4 Implement `IdTransformer` for TypeScript: strip `scip-typescript npm <pkg> <ver> ` prefix; preserve `<package>/<file-without-ext>.Symbol` form; document file-rename = ID-change limitation
- [x] 1.5 Implement `IdTransformer` for Rust: strip `rust-analyzer cargo <crate> <ver> ` prefix; produce `crate::path::to::item`; handle `impl#[Type][Trait]` impl-symbol pattern (transformed into the impl-block symbol's ID)
- [x] 1.6 Implement `IdTransformer` for Go: strip `gomod <package_path> <ver> ` prefix; produce `package_path.Symbol` or `package_path.Type.Method`
- [x] 1.7 Implement `IdTransformer` for Python: strip `python <distro> <ver> ` prefix; produce `module.Class.method`
- [x] 1.8 Test: round-trip — every fixture SCIP symbol parses to a `PublicId` that re-emits a stable canonical form (idempotent)
- [x] 1.9 Test: collision regression — different SCIP symbols never produce the same public ID across a fixture corpus per language
- [x] 1.10 Test: stability across fixture-pair where the file path changes but the semantic location doesn't — public ID is identical

## 2. Schema definition (DB-agnostic, expressed as data-model types)

- [x] 2.1 Define `Kind` enum (closed set per design D7): `Package`, `Module`, `Namespace`, `Class`, `Struct`, `Interface`, `Trait`, `Enum`, `EnumMember`, `TypeAlias`, `Method`, `Function`, `Constructor`, `Destructor`, `Operator`, `Field`, `Property`, `Constant`, `Variable`, `Parameter`, `TypeParameter`, `Macro`
- [x] 2.2 Define `FileRecord { id: u32, path: String, language: Language, is_test: bool, is_external: bool, content_hash: u64 }`
- [x] 2.3 Define `SymbolRecord` with all fields per design D5 (short_id, id, language, kind, name, display_name, enclosing_symbol u32, file u32, def_range [u32;4], is_partial bool, args_arity u8, generic_arity u8, is_external bool, is_test bool)
- [x] 2.4 Define `SymbolDocsRecord { symbol: u32, signature_doc: String, documentation: String }`
- [x] 2.5 Define `PartialDefRecord { symbol: u32, file: u32, range: [u32; 4] }`
- [x] 2.6 Define `EdgeKind` enum: `Calls`, `TypeUse`, `FieldAccess`, `Implements`, `Overrides`, `Instantiates`, `GenericConstraint`, `DefinedIn`, `Contains`, `Imports`, `CorrespondsTo`
- [x] 2.7 Define `EdgeRecord { kind: EdgeKind, source: u32, target: u32, properties: EdgeProperties }` where `EdgeProperties` is per-kind: `FieldAccess { op: Read|Write }`, `Imports { kind: Explicit|ReExport }`, `CorrespondsTo { source: Config|AutoInferred|Codegen, generator: String, canonical: u32 }`, others empty
- [x] 2.8 Validate: `EdgeKind` covers every relation listed in design D8/D9/D10/D11/D12

## 3. SurrealDB schema mapping

- [x] 3.1 Author SurrealQL `DEFINE TABLE` statements for `files`, `symbols`, `symbol_docs`, `partial_defs`
- [x] 3.2 Author SurrealQL `DEFINE FIELD` statements with default values (no nullable columns; sentinel `0` for u32 absent, `0` for u8 absent)
- [x] 3.3 Author SurrealQL `DEFINE TABLE TYPE RELATION` statements for `defined_in`, `contains`, `calls`, `type_use`, `field_access`, `implements`, `overrides`, `instantiates`, `generic_constraint`, `corresponds_to`, `imports`
- [x] 3.4 Author SurrealQL `DEFINE FIELD` statements for relation properties (`field_access.op`, `imports.kind`, `corresponds_to.source/generator/canonical`)
- [x] 3.5 Author SurrealQL `DEFINE INDEX` statements per design D16
- [x] 3.6 Author the BM25 analyzer definitions for `symbols.name` and `symbol_docs.documentation`
- [x] 3.7 Test: schema applies cleanly to an empty SurrealDB instance with no errors
- [x] 3.8 Test: a single round-trip insert+fetch of one symbol with one defined_in edge succeeds with all defaults applied

## 4. Wire location format

- [x] 4.1 Implement `format_location(file_path: &str, range: [u32; 4]) -> String` producing `./{path}#{start_line}` or `./{path}#{start_line}-{end_line}` (suppress trailing range when start==end)
- [x] 4.2 Implement `parse_location(s: &str) -> Result<(String, Range)>` for re-ingesting locations passed back by agents
- [x] 4.3 Test: round-trip — `parse(format(p, r))` matches `(p, r)` for a fixture set including single-line, multi-line, and column-positioned ranges (column ignored; round-trip preserves only line range)
- [x] 4.4 Test: format produces `null` (string `"null"` or JSON null per API convention) when file=0 / no def location

## 5. Test-file glob configuration

- [x] 5.1 Define `TestsConfig { paths: Vec<GlobPattern> }` parsed from `kenn.toml` `[tests]` section
- [x] 5.2 Implement `is_test_file(path: &str, config: &TestsConfig) -> bool` matching against the configured globs
- [x] 5.3 At ingest, populate `files.is_test` from this check; denormalize onto `symbols.is_test` for every symbol in that file
- [x] 5.4 Test: representative globs (`tests/**`, `**/*Test.cs`, `**/*_test.go`, `**/test_*.py`, `**/*.test.ts`, `**/*.spec.ts`) classify a fixture set correctly
- [x] 5.5 Test: empty `[tests]` section yields no test files (default-off)

## 6. Symbol-kind classifier (cross-cutting with scip-indexing-pipeline)

- [x] 6.1 Coordinate with `scip-indexing-pipeline` task 5c (descriptor classifier): the classifier outputs values from this proposal's `Kind` enum
- [x] 6.2 Test: scip-go and rust-analyzer's emitted `SymbolInformation.kind` values map deterministically to this proposal's `Kind` enum (per-indexer mapping table)

## 7. Documentation

- [x] 7.1 Document the public ID format with examples per language in the spec (already in design.md D1; carry over)
- [x] 7.2 Document the wire location format with examples (already in design.md D13; carry over)
- [x] 7.3 Document the deferred capabilities list (data flow, exception edges, snippet blob cache, codegen-detected isomorphism, suggestion engine for 404s) so future proposals can pick them up

## 8. Cross-reference cleanup

- [x] 8.1 Add a cross-reference in `indexed-store-and-lifecycle/design.md`: schema details defined in `source-data-model`. Storage layout, atomicity, ingest pipeline remain in indexed-store-and-lifecycle.
- [x] 8.2 Confirm no schema duplication between the two proposals after the cross-reference lands.
- [x] 8.3 Update `scip-indexing-pipeline/design.md` to point to `source-data-model` for the `Kind` enum, `EdgeKind` enum, and public ID format. Producer-side data model (`kenn-data-model` capability) becomes a *write* view of the schema defined here.
