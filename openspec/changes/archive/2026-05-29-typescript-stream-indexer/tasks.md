# Tasks — typescript-stream-indexer

## 1. Producer skeleton + distribution
- [x] 1.1 Scaffold `indexers/kenn-ts/` (bun project; `typescript` pinned) → verify: `bun run` prints the CLI help
- [x] 1.2 Import `indexers/frames.ts` as the frame types → verify: a hand-written `MetaFrame` type-checks against the schema
- [x] 1.3 CLI: `kenn-ts index --workspace <ws> [--tsconfigs …]` (+ batching/flush flags mirroring the wire constants) → verify: emits `meta` … `end` for an empty project
- [x] 1.4 Buffered stdout JSONL sink (flush by `DEFAULT_FLUSH_BYTES` / `DEFAULT_FLUSH_FRAMES`) → verify: one JSON object per line, valid JSON
- [x] 1.5 `just build-indexer-ts` → `bun build --compile` → `build/kenn-ts`; re-sign if needed → verify: warm startup runs, prints `end` frame

## 2. Discovery + program graph
- [x] 2.1 `tsconfig.json` discovery honoring explicit-exclude globs + git-worktree exclusion → verify: a duplicate worktree's tsconfigs are skipped
- [x] 2.2 `ts.createProgram` + shared `CompilerHost` cache across projects → verify: shared files parsed once across two projects
- [x] 2.3 Per-source-file AST walk producing `FileFrame` (test flag) → verify: file count matches in-set source files
- [x] 2.4 `FileFrame.content_hash` via `Bun.hash.xxHash64` → 16-hex lowercase (XXH64, confirmed by spike) → verify: matches the known vector `ef46db3751d8e999` for empty input

## 3. Symbols, packages, ids
- [x] 3.0 Extend wire `SymbolKind` with `function` + `enum_member` (frames.ts + kenn-dotnet `SymKind.cs` mirror + `transform_jsonl` `kind_from_str` → `Kind::Function` / `Kind::EnumMember`); other TS constructs reuse existing kinds (type alias→`type`, `let`/`var`→`symbol`) → verify: `function`→`Kind::Function`, `enum_member`→`Kind::EnumMember` (matching Rust/Go); no type-param symbols emitted
- [x] 3.0b kenn-dotnet `KindMap`: emit `SymKind.EnumMember` for enum-member fields (`IFieldSymbol` whose `ContainingType.TypeKind == Enum`) before the `IsConst ? Const : Field` fallback → verify: a C# enum member now ingests as `Kind::EnumMember` (was `Constant`); a C# `const` field is still `Constant`
- [x] 3.1 Descriptor/moniker `key` builder (suffix scheme) → verify: a type and same-named value get distinct keys
- [x] 3.2 `PackageFrame` from nearest `package.json`; `external`/`manager` flags → verify: workspace pkg vs node_modules pkg classified correctly
- [x] 3.3 `IdRegistry`: intern `ts.Symbol`→`Ref`, monotonic from 1 → verify: stable within a run; 0 never assigned
- [x] 3.4 `SymbolFrame` for definitions (kind, name, parent, file, range, sig, doc, nargs, targs); locals excluded → verify: a method-local var emits no symbol
- [x] 3.5 `StubFrame` for forward refs (upgrade to SymbolFrame, same Ref) and external symbols (.d.ts in node_modules, no upgrade) → verify: external symbol has a stub, no symbol, `external` pkg
- [x] 3.6 Emit one `module` `SymbolFrame` per module-file (import/`contains`/`defined_in` anchor); script files with no import/export get none (D13) → verify: a module file has a module symbol parenting its top-level decls; a global script file has none

## 4. Edges — full parity
- [x] 4.1 Structural: `defined_in`, `contains` → verify: member→type→namespace chain and module→file
- [x] 4.2 `imports` (import / export-from / re-export, de-aliased) → verify: barrel re-export resolves to original symbol
- [x] 4.3 `calls` (range = call site) → verify: call to a function emits one `calls` edge
- [x] 4.4 `type_use` (annotations, returns, generic positions) → verify: a typed param emits `type_use`
- [x] 4.5 `field_access` + `field_op` (read/write via assignment context) → verify: `x.f = 1` → write; `y = x.f` → read
- [x] 4.6 `instantiates` (type arguments) → verify: `new Map<K,V>()` emits `instantiates` for K and V
- [x] 4.7 `implements` / `overrides` — explicit heritage clauses only → verify: `class C implements I` emits `implements`; structural-only conformance emits none
- [x] 4.8 `generic_constraint` (type-param `extends`) → verify: `<T extends Base>` emits the edge

## 5. Declaration merging + file docs
- [x] 5.1 Declaration merging → `partial: true`, distinct Refs, shared `(key, pkg)` → verify: merged interface+namespace appears as multiple sites, edges from each preserved
- [x] 5.2 Function overloads → multiple partial sites → verify: each overload signature is a site
- [x] 5.3 File-level docs: leading comment blocks → `FileFrame.doc` (raw, unfiltered) → verify: header `//` block + `/** @fileoverview */` captured; shebang skipped; JS file with no header emits no doc

## 6. Integration + swap
- [x] 6.1 `KennTs` `JsonlIndexer` driver spawning `build/kenn-ts` → verify: ingests through the pipeline, TS symbols/edges land in the store
- [x] 6.2 Register `KennTs`; remove `with_scip_driver(ScipTypescript)`; delete `ScipTypescript` + its `scip-indexer` registry entry → verify: `kenn index` over a TS workspace produces typed edges
- [x] 6.3 Reindex a real TS workspace; diff graph vs the old SCIP path → verify: edge kinds present (calls/type_use/field_access/…); file docs searchable as file hits
- [x] 6.4 Update `scip-indexer` + `jsonl-indexer-driver` specs (TS removed from SCIP; kenn-ts in JSONL registry)
- [x] 6.5 Extend `kenn.toml` `[language.typescript]` with `kenn_ts_path` (default `build/kenn-ts`); `KennTs` driver reads it (D16) → verify: driver locates the executable via config
- [x] 6.6 Delete the SCIP TypeScript path: `TypeScriptTransformer`, the empty-`Document.language` fallback, and TS branches in `is_test`/`language_from_path` (D15) → verify: workspace builds clean, no dead-code warnings, TS still indexes via kenn-ts

## 7. Quality gates
- [x] 7.1 Producer unit tests (per edge kind, decl-merging, file-doc, key stability) → verify: green
- [x] 7.2 Rust side: `cargo clippy --workspace --all-targets` clean, `just crap-ci` PASSED, `cargo fmt --all`
- [x] 7.3 Whole-repo timing check on a real TS monorepo → verify: within the ~20s spike envelope
