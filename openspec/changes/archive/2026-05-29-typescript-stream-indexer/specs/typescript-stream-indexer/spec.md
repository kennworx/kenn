## ADDED Requirements

### Requirement: TypeScript JSONL producer conforming to the shared wire

The `kenn-ts` producer SHALL emit the `indexers/frames.ts` JSONL wire on stdout — `meta` first, `end` last, one JSON object per line — with `MetaFrame.language = "typescript"`. The producer SHALL depend on `indexers/frames.ts` as its frame type definitions (importing them directly) rather than maintaining a separate copy, so the producer is compile-time-checked against the canonical schema. Module layout is otherwise unconstrained.

#### Scenario: Frames conform to the shared schema

- **WHEN** `kenn-ts index` runs over any TypeScript project
- **THEN** every emitted line is a single valid JSON object whose `type` is one of the `Frame` union members, beginning with one `meta` (language `"typescript"`) and ending with one `end`

#### Scenario: Empty project still brackets the run

- **WHEN** a discovered project contains no indexable source files
- **THEN** the producer still emits exactly one `meta` and one `end` with zeroed stats

### Requirement: Compiler-API discovery and resolution

The producer SHALL discover indexable units by locating `tsconfig.json` projects in the workspace, skipping files under the configured explicit-exclude globs and under any other-worktree directories (the same exclusion rules as the SCIP discovery path). It SHALL resolve symbols and types using the TypeScript compiler API (`ts.createProgram` + `TypeChecker`). The producer SHALL be invoked at most once per workspace per run and owns all scheduling across the projects it discovers; it MAY share a compiler host cache across projects.

#### Scenario: Duplicate git worktree is excluded

- **WHEN** the workspace contains a linked git worktree with its own `tsconfig.json` files
- **THEN** those worktree tsconfigs are not indexed

#### Scenario: Single invocation owns project scheduling

- **WHEN** a workspace contains multiple `tsconfig.json` projects
- **THEN** the pipeline invokes `kenn-ts` exactly once and the producer indexes all projects within that one process

### Requirement: Cross-run-stable descriptor keys

Each `SymbolFrame`/`StubFrame` `key` SHALL be a language-naked, intra-package descriptor path built from the symbol's declaration chain using suffixes that distinguish symbol roles (namespace, type, term, method with disambiguator, parameter, type-parameter, meta). A type and a value sharing the same name SHALL receive distinct keys via their differing suffix.

#### Scenario: Type/value namespace collision yields distinct keys

- **WHEN** a file declares both a type and a value with the same identifier (e.g. `interface Foo` and `const Foo`)
- **THEN** the two symbols are emitted with distinct `key`s

### Requirement: Packages resolved from package.json

The producer SHALL resolve each symbol's owning package by walking up from its source file to the nearest `package.json`, emitting one `PackageFrame` per `(name, version)` before any frame references it. Packages resolved outside the workspace (into `node_modules`/lib) SHALL be marked `external: true` with `manager: "npm"`; workspace-local packages SHALL omit `external`.

#### Scenario: Workspace vs external package classification

- **WHEN** a symbol is defined in a workspace package and another is imported from a `node_modules` dependency
- **THEN** the workspace package frame omits `external` and the dependency package frame sets `external: true`

### Requirement: Full edge taxonomy at parity with C#

The producer SHALL classify each reference site from its AST context and emit the precise edge kind: `calls` (callable invocation, `range` = call site), `type_use` (type positions), `field_access` (property/field read or write, with required `field_op`), `instantiates` (generic type arguments), `implements` (explicit heritage), `overrides` (explicit member override), `generic_constraint` (type-parameter `extends`), `imports` (import/export-from/re-export), `defined_in` (enclosing-symbol), and `contains` (module/namespace → file). The producer SHALL NOT collapse these to a single generic reference.

#### Scenario: Reference kinds are distinguished

- **WHEN** a function is called, a type is used in an annotation, and a property is read
- **THEN** the producer emits a `calls`, a `type_use`, and a `field_access` (`field_op: "read"`) edge respectively — not three identical references

#### Scenario: Field write is classified

- **WHEN** a property access appears on the left-hand side of an assignment or as a `++`/`--` target
- **THEN** the `field_access` edge carries `field_op: "write"`

### Requirement: Structural conformance is not an edge

The producer SHALL emit `implements`/`overrides` edges ONLY for explicit `extends`/`implements` heritage clauses and explicit member overrides. Implicit structural-typing conformance SHALL NOT produce edges.

#### Scenario: Explicit clause emits, duck typing does not

- **WHEN** one class declares `implements I` and another merely has a compatible shape without an `implements` clause
- **THEN** only the explicit declaration emits an `implements` edge

### Requirement: Declaration merging via the partial mechanism

For symbols with multiple merged declaration sites (interface merging, namespace merging, function overloads, enum+namespace), the producer SHALL emit one `SymbolFrame` per site with `partial: true` and a distinct `Ref` sharing the same `(key, pkg)`, so the consumer dedup-appends sites without dropping per-site edges.

#### Scenario: Merged interface across sites

- **WHEN** an interface is declared in two places (or a function has multiple overload signatures)
- **THEN** each site is emitted as a `partial` symbol with the same `(key, pkg)` and distinct `Ref`, and edges from each site are preserved

### Requirement: External and forward references use stubs

The producer SHALL emit a `StubFrame` for a forward reference and later upgrade it to a `SymbolFrame` reusing the same `Ref`. Symbols resolved into external `.d.ts` (node_modules/lib) SHALL be emitted as a `StubFrame` only, never upgraded; the consumer derives `external` from the resolved package.

#### Scenario: External library symbol is a stub

- **WHEN** code references an export from a `node_modules` dependency
- **THEN** that symbol appears as a `StubFrame` against an `external` package and never as a `SymbolFrame`

### Requirement: File-level comment docs

The producer SHALL emit leading file comment blocks on `FileFrame.doc` (raw, unfiltered): contiguous `//` lines coalesced into one block, blank line breaking blocks, each `/* */`/`/** */` as one block, with a leading `#!` shebang line skipped. License-boilerplate filtering is a consumer concern. Files with no leading comments SHALL omit `doc`.

#### Scenario: Header comments captured, shebang skipped

- **WHEN** a file begins with a `#!` shebang followed by a `/** @fileoverview … */` block
- **THEN** `FileFrame.doc` contains the file-overview block and not the shebang line

#### Scenario: No header emits no doc

- **WHEN** a source file has no leading comment
- **THEN** `FileFrame.doc` is omitted

### Requirement: Locals are not emitted as symbols

The producer SHALL NOT emit `SymbolFrame`s for method-local variables, lambda/arrow parameters, block-scoped bindings, or anonymous types.

#### Scenario: Local variable produces no symbol

- **WHEN** a function body declares a local `const`
- **THEN** no `SymbolFrame` is emitted for it

### Requirement: Per-file module symbol anchors containment and imports

The producer SHALL emit one `module` `SymbolFrame` per module-file (a file with any `import`/`export`). It serves as the `imports` endpoint (module → module), the `contains` source (module → file), and the `parent` of the file's top-level declarations. A global script file with no import/export SHALL NOT receive a module symbol; its top-level declarations carry `parent: 0`.

#### Scenario: Module file gets a module symbol

- **WHEN** a file has at least one `import` or `export`
- **THEN** the producer emits a `module` symbol for it, the file's top-level declarations name it as `parent`, and a `contains` edge links it to the file

#### Scenario: Global script file gets no module symbol

- **WHEN** a `.ts`/`.js` file has no import or export (global scope)
- **THEN** no `module` symbol is emitted and its top-level declarations carry no parent

### Requirement: Symbol kinds cover TypeScript constructs

The wire `SymbolKind` set SHALL be extended additively with `function` (→ `Kind::Function`) and `enum_member` (→ `Kind::EnumMember`), since for JSONL symbols the wire `kind` field is authoritative and no existing kind yields those — yet both are the cross-language standard (Rust and Go already produce them). The producer SHALL emit the most specific kind for every construct: function → `function`, enum member → `enum_member`, type alias → `type`, top-level `let`/`var` → `symbol`, `const` → `const`, get/set accessor → `accessor`, method → `method`. The producer SHALL NOT emit type-parameter symbols (type parameters are not symbol endpoints; `generic_constraint` is sourced from the owner).

#### Scenario: Top-level function is a function, not a method

- **WHEN** a module exports a top-level `function foo()`
- **THEN** its `SymbolFrame.kind` is `function` and the consumer resolves it to `Kind::Function` (not `Kind::Method`)

#### Scenario: Enum member matches the Rust/Go standard

- **WHEN** an `enum` declares members
- **THEN** each member is emitted with kind `enum_member` → `Kind::EnumMember` (consistent with the Rust and Go indexers)

### Requirement: Single-file executable distribution

`kenn-ts` SHALL be built into a single-file executable via `bun build --compile` (embedding the bundled `typescript` compiler), invoked by the pipeline as a spawned process exactly like the C# indexer.

#### Scenario: Compiled executable indexes a workspace

- **WHEN** the `build/kenn-ts` executable is invoked with `index --workspace <ws>`
- **THEN** it streams a conforming JSONL frame sequence on stdout without requiring a separate Node/bun project install
