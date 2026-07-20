## Why

TypeScript is indexed today via `scip-typescript` → SCIP protobuf → ingest. That path is **structurally lossy**, and the loss cannot be recovered downstream:

- **The edge taxonomy collapses.** `scip-typescript` emits only occurrences — `(range, symbol, role ∈ {definition, reference})`. kenn's C# graph carries a 10-kind typed taxonomy (`calls`, `type_use`, `field_access` with read/write `field_op`, `instantiates`, `implements`, `overrides`, `generic_constraint`, `imports`, `defined_in`, `contains`). You cannot reconstruct "this reference was a *call* vs a *type use* vs a *field write*" from a bare SCIP reference — that classification only exists at index time, with the AST and type-checker in hand. So kenn's TS graph is permanently coarser than its C# graph.
- **File-level comments are dropped.** `scip-typescript` only attaches synthetic signature fences and checker-resolved JSDoc to symbols. File headers, license blocks, top-of-file `/* */`, and bare `/** @fileoverview */` never reach the index. (Verified empirically against a real TS monorepo: 0 of ~23.8k symbol-documentation strings carried any file-header text.)
- **Plain `//` comments are dropped** — only `/** */` JSDoc the checker binds to a symbol survives.

We want TypeScript at **full parity with C#** — the same typed edges, file-level docs, and read/write field ops. The wire format already anticipates this (`indexers/frames.ts` lists `"typescript"` as a producer language and `kenn-dotnet` is the reference implementation of "custom indexer → JSONL frames"). This change adds a second JSONL producer, `kenn-ts`, and retires the SCIP path for TypeScript.

Two spikes de-risk it (numbers in `design.md`):
- **Checker cost is ~neutral.** A parity-style walk of the heaviest project (75 files) was ~4.0s vs `scip-typescript`'s 3.8s — the dominant cost is `getSymbolAtLocation`-per-identifier, which `scip-typescript` *already* pays; the extra `getTypeAtLocation` calls for edge classification are nearly free.
- **`bun build --compile` works.** Bundling the `typescript` compiler into a single-file executable produces a ~69 MB binary with ~100 ms warm startup — the same single-file-binary distribution model as `kenn-dotnet`.

## What Changes

- **New producer `kenn-ts`**: a from-scratch TypeScript indexer (free layout; the only contract is the JSONL wire). Built on the TypeScript compiler API (`ts.createProgram` + `TypeChecker`), it discovers `tsconfig.json` projects, walks each source file's AST, and **streams `frames.ts` frames on stdout** — importing `indexers/frames.ts` directly so the producer is type-checked against the canonical schema (no hand-mirrored copy, unlike C#'s `Frames.cs`).
- **Full edge parity from the start**: an AST-context → `EdgeKind` classifier emits `calls` / `type_use` / `field_access` (+`read`|`write`) / `instantiates` / `implements` / `overrides` / `generic_constraint` / `imports` / `defined_in` / `contains`. The narrow defs/refs floor is *not* re-implemented — `scip` already covers that and we replace it wholesale.
- **File-level docs**: leading comment trivia (`ts.getLeadingCommentRanges`) emitted on `FileFrame.doc`, feeding the existing license-filter + `file_docs` path built for C#/Rust.
- **Distribution**: `bun build --compile` → single-file `build/kenn-ts`, spawned by the pipeline exactly like `build/kenn-dotnet`.
- **Driver swap**: register `KennTs` as a second `JsonlIndexer`; drop the `ScipTypescript` `ScipDriver`. No pipeline change — the runner already loops over `Vec<JsonlIndexer>` and gives each its own `IdRegistry` language partition.

## Capabilities

### New Capabilities
- `typescript-stream-indexer`: the `kenn-ts` producer — TypeScript-compiler-API-based discovery, symbol/edge collection at full parity, declaration-merging handling, file-doc extraction, and JSONL frame emission conforming to the shared wire.

### Modified Capabilities
- `scip-indexer`: TypeScript is removed from SCIP discovery and the indexer-command registry (it moves to the JSONL path). C#-via-scip-dotnet, Rust-via-rust-analyzer, etc. are unaffected.
- `jsonl-indexer-driver`: the JSONL driver registry now includes `kenn-ts` alongside `kenn-dotnet`; the per-workspace single-invocation and per-driver `IdRegistry`-partition contracts already cover N drivers unchanged.
- `dotnet-stream-indexer`: `kenn-dotnet` emits `enum_member` for enum members (was `const`), adopting the shared wire's new kind so all languages classify enum members uniformly.

## Impact

- **Wire format**: additive only — `FileFrame.doc` and the full `EdgeKind` set already exist; `SymbolKind` gains `function` (→ `Kind::Function`) and `enum_member` (→ `Kind::EnumMember`), both already the cross-language standard (Rust/Go produce them) but unreachable through the JSONL wire today. Other TS constructs reuse existing kinds (type alias → `type`, `let`/`var` → `symbol`). No version bump; the `kenn-dotnet` `SymKind.cs` mirror is kept in sync. `content_hash` is computed via `Bun.hash.xxHash64` (XXH64, byte-identical to the C# producer — confirmed by spike).
- **C# enum-member kind changes**: `kenn-dotnet` enum members now classify as `Kind::EnumMember` instead of `Kind::Constant` — a cross-language-consistency fix riding on the shared `enum_member` wire value. Existing C# snapshots regenerate by reindex.
- **New symbol rows for TS files**: `kenn-ts` emits one `module` symbol per module-file (the `imports`/`contains`/`defined_in` anchor); script files with no import/export get none.
- **SCIP-TS code removed**: `TypeScriptTransformer`, the `scip-typescript 0.4.0` empty-language fallback, and TS branches in `is_test`/`language_from_path` are deleted (git retains history).
- **Config**: `kenn.toml` `[language.typescript]` gains `kenn_ts_path` (default `build/kenn-ts`).
- **Graph quality**: TS edges go from one collapsed "reference" kind to the full typed taxonomy — a large jump in TS code-intelligence fidelity. Existing snapshots are regenerated by a reindex.
- **New build artifact + toolchain**: `build/kenn-ts` via `bun build --compile`; a new `just build-indexer-ts` recipe and a bundled `typescript` dependency. Adds bun to the indexer build path (already used elsewhere in the repo).
- **Dependency on `scip-typescript` removed** for indexing; the binary/`bunx` invocation and its driver are deleted.
- **Out of scope**: non-TS JSONL producers (Rust/Go/Python stay on SCIP); structural-typing conformance edges (only explicit `extends`/`implements` clauses are edges — see design); indexing `node_modules` sources (external symbols remain stubs).
