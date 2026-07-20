# Design — TypeScript streaming indexer (`kenn-ts`)

## Spike evidence (measured, not assumed)

Run against a real-world TypeScript monorepo (~555 files across 15 `tsconfig` projects, plus a duplicate git worktree that canonicalization already excludes).

**Spike 1 — checker cost is ~neutral vs the SCIP path.** Parity-style walk of the heaviest project (75 files), resolving the symbol at every identifier and the type at every call/property-access target:

```
  program create            760 ms
  checker init              153 ms
  structural walk (no checker)  2 ms     ← parsing/AST is effectively free
  walk + checker queries   3081 ms       ← 21,626 symbol + 9,976 type queries
  ────────────────────────────────
  total                    ~4.0 s        vs scip-typescript 3.8 s for the same project
```
The cost is `getSymbolAtLocation`-per-identifier, which `scip-typescript` already pays to build occurrences. The extra `getTypeAtLocation` calls that distinguish edge kinds are nearly free. Whole-repo extrapolation ≈ ~20s, same ballpark as today. **Conclusion: full parity is affordable; no need to ration checker queries by node kind in v1.**

**Spike 2 — `bun build --compile` bundles `typescript`.**
```
  bundle + compile:  ~0.3 s   (typescript + entry → single file)
  artifact:          69 MB    (embeds bun runtime + tsc; same order as kenn-dotnet)
  startup:           cold 2.27 s (one-time), warm ~0.10 s
  ts API reachable inside the exe: yes (ts.version = 5.6.2)
```
**Conclusion: single-file `build/kenn-ts` is viable; warm startup is negligible and the pipeline spawns it once per run.**

## Decisions

### D1. Fresh layout; the JSONL wire is the only contract
`kenn-ts` does **not** mirror `kenn-dotnet`'s C# module layout. It adopts whatever structure reads cleanly in TypeScript. Conformance is defined solely by the emitted frame stream matching `indexers/frames.ts`. The producer **imports `indexers/frames.ts` directly** as its frame types — unlike `kenn-dotnet`, which hand-mirrors the schema in `Frames.cs` and can drift. The TS producer is therefore compile-time-checked against the canonical schema; `frames.ts` becomes the single source of truth it depends on.

### D2. TypeScript compiler API, one shared program graph
Discovery and resolution use `ts.createProgram` + `program.getTypeChecker()`, the same machinery `tsc`/`scip-typescript` use — full type resolution for free. Project discovery finds `tsconfig.json`s (honoring the workspace's explicit-exclude globs and git-worktree exclusion, matching `scip-indexer`'s discovery rules). A shared `CompilerHost` cache across projects (as `scip-typescript`'s `ProjectIndexer` does) avoids re-parsing shared files in a monorepo. The indexer owns all scheduling within its single process invocation (per `jsonl-indexer-driver`).

### D3. Symbol identity / `key` — descriptor moniker
The wire `key` is the language-naked, intra-package, cross-run-stable descriptor path. `kenn-ts` builds it from the symbol's declaration chain using SCIP-style descriptor suffixes (namespace `/`, type `#`, term `.`, method `(disambiguator).`, parameter `(x)`, type-parameter `[x]`, meta `:`) — the same scheme `scip-typescript`'s `Descriptor`/`ScipSymbol` use, so monikers stay stable and comparable. The **suffix disambiguates the type-vs-value namespace collision** automatically: `class Foo` / `type Foo` (a `#`) and `const Foo` / `function Foo` (a `.`) intern as distinct keys with no special-casing.

### D4. Package frames from `package.json`
Each symbol's owning package is resolved by walking up from its source file to the nearest `package.json` (name + version), emitted once per `(name, version)` as a `PackageFrame`. Workspace-local packages omit `external`; everything resolved into `node_modules`/lib is `external: true`, `manager: "npm"`. Falls back to `HEAD` (missing version) or an anonymous package, matching `scip-typescript`'s `Packages` behavior.

### D5. ID registry + stubs, per-language partition
`kenn-ts` assigns monotonic u32 `Ref`s starting at 1 (0 = none), interning `ts.Symbol` → `Ref`. Forward references and external symbols emit a `StubFrame` (`kind`, `name`, `key`, `pkg`); an internal forward ref later upgrades to a `SymbolFrame` reusing the **same `Ref`**. External symbols (resolved into `node_modules`/lib `.d.ts`) emit a `StubFrame` only and never upgrade — the consumer derives `external` from the package flag. The pipeline already gives each JSONL driver its own `IdRegistry`, so `kenn-ts` Refs never collide with `kenn-dotnet` Refs.

### D6. Edge classification — the core new work (AST context → EdgeKind)
At each reference site the classifier inspects the AST parent context and the resolved symbol to emit the precise edge (mirrors `kenn-dotnet`'s `BodyWalker`, IndexerCore.cs:798-990, in TS terms):

```
  CallExpression.expression resolves to a callable     → calls         (range = call site)
  type position (annotations, return types, generics)  → type_use
  PropertyAccess / ElementAccess on a field/property   → field_access  (+ field_op)
  type arguments on a generic instantiation            → instantiates
  `extends` / `implements` heritage clause             → implements
  method overriding a base/interface member            → overrides
  type-parameter `extends` constraint                  → generic_constraint
  import / export-from / re-export                     → imports
  enclosing-symbol relationship                        → defined_in
  module/namespace → file                              → contains
```
`field_op` (read vs write) is classified from assignment context exactly as `kenn-dotnet`'s `ClassifyFieldOp` (IndexerCore.cs:967-990): assignment LHS, compound-assignment target, and pre/postfix `++`/`--` → `write`; otherwise `read`. Edge endpoints are always introduced (as stubs at minimum) before the edge frame.

### D7. Structural typing → only explicit heritage clauses are edges
TypeScript is structurally typed; duck-typed conformance is unbounded and not what a user means by "implements". `kenn-ts` emits `implements`/`overrides` edges **only** for explicit `extends`/`implements` clauses and explicit member overrides. Implicit structural conformance is deliberately not an edge. Documented boundary, not a gap.

### D8. Declaration merging → the `partial` mechanism
TypeScript merges declarations (interface+interface, namespace+namespace, function overloads, enum+namespace, the interface+namespace+function trio). Each declaration site emits one `SymbolFrame` with `partial: true` and a **distinct `Ref`** sharing the same `(key, pkg)`; the consumer's `(key, pkg)` dedup appends additional sites without dropping per-site edges. This is the exact mechanism the wire defines for C# `partial class` / Rust `impl` (frames.ts:244-248) — declaration merging is its TS instance.

### D9. File-level docs via leading comment trivia
For each in-source file, `ts.getLeadingCommentRanges` over the file prefix yields the header comment region; contiguous `//` lines coalesce into one block, a blank line breaks the block, `/* */` and `/** */` are one block each, and a leading `#!` shebang line is skipped. These blocks are emitted on `FileFrame.doc` (per the existing field, frames.ts:142-146) — raw, unfiltered; the consumer's license-boilerplate filter and `file_docs` dataset (built for C#/Rust) handle them unchanged.

### D10. Locals are not symbols
Method-local variables, lambda/arrow params, block-scoped bindings, and anonymous types are never emitted as `SymbolFrame`s (per the wire), matching `scip-typescript`'s `local <n>` handling and `kenn-dotnet`. They may still anchor occurrences/edges to their declared symbol but get no symbol record.

### D11. Documentation + signature on symbols
`SymbolFrame.sig` is the rendered type signature (from the checker), and `SymbolFrame.doc` is the symbol's JSDoc (`getDocumentationComment`) — i.e. at least what `scip-typescript` already provides, so symbol-level search text does not regress.

### D12. Distribution + driver swap
`bun build --compile src/main.ts --outfile build/kenn-ts` produces the single-file executable; a `just build-indexer-ts` recipe parallels `build-indexer-dotnet`. A new `KennTs` `JsonlIndexer` in the driver spawns it (`kenn-ts index --workspace <ws> [--tsconfigs …]`), streaming JSONL on stdout. Registration drops `with_scip_driver(ScipTypescript)` and adds `with_jsonl_driver(KennTs)`; `ScipTypescript` and its `scip-indexer` registry entry are deleted.

### D13. One `module` SymbolFrame per module-file
TypeScript modules *are* files, so `kenn-ts` SHALL emit one `SymbolFrame` of kind `module` per file that is a module (has any `import`/`export`), serving three roles the edge taxonomy needs:
- **`contains`**: `module → file` (the module symbol contains its file).
- **`imports`**: `module → module` — an `import`/`export-from`/re-export links the importing file's module symbol to the imported module symbol (an internal file's module, or an external module stub for `node_modules`/lib).
- **`defined_in`**: top-level declarations in the file get the module symbol as `parent`.

Script files with **no** import/export (global-scope `.js`/`.ts`) get **no** module symbol (mirrors `scip-typescript`'s `isEmpty()` guard); their top-level symbols carry `parent: 0` and the file is reached via package `contains`. The module symbol's `key` is the module-path descriptor (namespace suffix), stable across runs.

### D14. Extend the wire `SymbolKind` minimally — `function` only
For **JSONL symbols the wire `kind` field is authoritative** (`kind_from_str`, transform_jsonl.rs:689); the descriptor-suffix `kind_classifier` is only the SCIP-path fallback (for indexers that leave `SymbolInformation.kind` unset). So the lever to get a precise `Kind` for a kenn-ts symbol is the wire `kind` string, not the `key` suffix.

The current JSONL `kind_from_str` map yields neither `Kind::Function` nor `Kind::EnumMember`, but the SCIP-based language paths already produce both: `kind_from_rust_analyzer_kind`/`kind_from_scip_go_kind` map `ScipKind::EnumMember → Kind::EnumMember` (kind_classifier.rs:173, 233) and likewise for `Function`. So `EnumMember`/`Function` are the cross-language standard — they're just unreachable through the JSONL wire today. Extend **additively** (no version bump — `SymbolKind` is referenced as a type, not normatively enumerated):
```
  add to frames.ts SymbolKind:        "function" | "enum_member"
  add to kenn-dotnet SymKind.cs:      Function, EnumMember   (keep the mirror)
  add to transform_jsonl kind_from_str:  "function" → Kind::Function ; "enum_member" → Kind::EnumMember
```

**Empirical basis for `enum_member` (checked):** Rust and Go both classify enum members/variants as `Kind::EnumMember` (kind_classifier.rs:173, 233). C#'s `kenn-dotnet` maps them to `Const → Kind::Constant` (KindMap.cs:34) — that is the **outlier**, not the standard. Rather than add a third behavior, **this change also fixes C#**: `kenn-dotnet`'s `KindMap` emits `SymKind.EnumMember` for a field whose containing type is an enum (`IFieldSymbol f when f.ContainingType?.TypeKind == TypeKind.Enum => SymKind.EnumMember`), before the existing `IsConst ? Const : Field` fallback. After this, all four languages (C#, TS, Rust, Go) classify enum members uniformly. (C# enum-member symbols change kind from `Constant` to `EnumMember`; no users, reindex regenerates.)

**Rejected additions (kept the wire small):**
- `type_parameter` — type params are never symbol endpoints. `generic_constraint` is sourced from the **owner**, not the type param (IndexerCore.cs:586), and uses of `T` resolve like locals (D10). No type-param symbols are emitted, so the kind would be dead.
- `variable` — `"symbol" → Kind::Variable` already exists; `let`/`var` reuse `"symbol"`. A new kind yields no new consumer `Kind`.

TS → wire kind mapping (`function` + `enum_member` are the additions):
```
  module decl/file-module → module        namespace decl   → namespace
  class                   → class          interface        → interface
  enum                    → enum           enum member      → enum_member (NEW)
  type alias              → type           function         → function    (NEW)
  method                  → method         get/set accessor → accessor
  property / field        → property/field const            → const
  let / var               → symbol         constructor      → constructor
```

### D15. Delete the SCIP TypeScript path (history in git)
Removing TS from SCIP makes `TypeScriptTransformer` (transform.rs:38), the `scip-typescript 0.4.0` empty-`Document.language` fallback (transform.rs:334), and the TS branches in `is_test`/`language_from_path` unreachable. **Delete them** — git retains the history if we ever want the SCIP-TS path back. No dormant dead code.

### D16. Extend config with a TS indexer path
`kenn.toml` `[language.typescript]` gains `kenn_ts_path` (default `build/kenn-ts`), parallel to `[language.csharp].kenn_dotnet_path`. The `KennTs` driver reads it to locate the executable.

### D17. Edges carry explicit enclosing-symbol source (bypasses the SCIP FROM heuristic)
Because `kenn-ts` emits `EdgeFrame`s directly with `source` = the enclosing symbol's `Ref` (tracked during the walk, as `kenn-dotnet`'s `BodyWalker` does), it **never** needs the SCIP path's `enclosing_range` FROM-attribution heuristic (the three-tier positional refinement in `scip-indexer`). Direct emission is both more accurate and the reason the typed taxonomy is expressible at all. The shared `aggregate` phase consumes these edges identically to C#'s.

## Build order (sequencing, not scope — target is full parity)
```
  1. skeleton: meta / file / package / symbol(defs) / end  → ingest through kenn, confirm TS symbols appear
  2. structural edges: defined_in, contains, imports + FileFrame.doc
  3. body edges: calls, type_use, field_access(±op), instantiates, implements, overrides, generic_constraint
  4. swap driver registration; delete ScipTypescript; reindex and diff graph against the old SCIP path
```

## Resolved by spike
- **`content_hash` is byte-identical with no extra dependency.** `Bun.hash.xxHash64("")` = `ef46db3751d8e999` — the canonical XXH64 seed-0 vector (XXH3 would be `2d06800538d394c2`), returned as a `bigint`. `value.toString(16).padStart(16, "0")` matches C#'s `BinaryPrimitives.ReadUInt64BigEndian(hash).ToString("x16")` (FileTracker.cs:164). So `kenn-ts` computes `content_hash` via `Bun.hash.xxHash64` directly.

## Open questions / risks (design-level, not feasibility)
- **Edge-classification fidelity** is the bulk of correctness risk: type-vs-value positions, aliased re-exports (de-alias through the checker to the original symbol, as `scip-typescript` does), and ambient declarations. Covered by per-edge-kind scenario tests against fixtures.
- **Method-call precedence**: `obj.method()` is a property access *then* a call — the classifier MUST emit `calls` on the method, not `field_access`, when the property access is the callee of a `CallExpression` (check the parent before classifying a property access as field access).
- **Default / namespace exports** (`export default`, `export =`, `import * as ns`): need stable `key`s; `scip-typescript` special-cases these — mirror its handling.
- **`allowJs` projects** index `.js` too; weaker types yield more `any` and coarser edges — accepted degradation.
- **`bun` in the build path** — confirm the bundled `typescript` version is pinned and the `--compile` artifact is re-signed if copied (macOS ad-hoc signature invalidation, as the CLI build already handles).
