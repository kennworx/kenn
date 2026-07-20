# code-intel-data-model

## Purpose

Defines the normalized representation of source-code structure (symbols, occurrences, relationships, file→file edges, project→project edges) that all indexers produce and all consumers read. Specifies identity rules (canonical path + symbol + range), edge kinds (extends/implements/calls/field_type/param_type/return_type/instantiates/imports/contains), how external-package references are represented, and provenance per indexer-run. Indexer-agnostic. Schema-level requirements (kind enum, identity, etc.) are aligned with the authoritative `source-data-model` capability; producer-specific requirements (path canonicalization, edge derivation, run-report counters) live here.
## Requirements
### Requirement: Symbol identity

The data model SHALL identify each distinct symbol occurrence by the composite key `(canonical_path, symbol_string, range)`. Equality on `symbol_string` alone MUST NOT be used as a dedup key, because indexers (e.g., scip-dotnet) emit identical local-package descriptors (`nuget . .`) for projects that share a root namespace.

#### Scenario: Two projects share a root namespace

- **WHEN** two distinct C# projects in the same workspace each define a class `Common.Helpers` and the indexer emits the same `symbol_string` for both
- **THEN** the data model MUST retain both as separate symbol entries keyed by their respective `canonical_path`s
- **AND** the merge layer MUST emit a structured warning identifying the collision and the two paths involved

#### Scenario: Same file indexed twice

- **WHEN** the same source file is indexed by two different indexer runs (e.g., two `.sln` files that both include the project) and both emit the same `(symbol_string, range)`
- **THEN** the data model MUST treat them as a single symbol entry (dedup), not two

### Requirement: Path canonicalization

The data model SHALL store paths in **workspace-relative form**, computed by combining each indexer's `metadata.project_root` with each document's `relative_path` to produce an absolute path, then re-rooting at the configured workspace root.

#### Scenario: Indexer rooted at a sub-directory

- **WHEN** an indexer reports `metadata.project_root = file:///workspace/sub` and a document `relative_path = "Foo/Bar.cs"`
- **AND** the configured workspace root is `/workspace`
- **THEN** the stored canonical path MUST be `sub/Foo/Bar.cs`

#### Scenario: Path outside the workspace root

- **WHEN** path canonicalization produces an absolute path not under the configured workspace root
- **THEN** the entry MUST be skipped and a structured warning emitted

### Requirement: Symbol kind enumeration

The data model SHALL define a closed set of symbol kinds covering: namespace, class, interface, struct, enum, record, method, constructor, field, property, event, parameter, type_parameter, local. Indexer-emitted kinds outside this set SHALL be mapped to the closest fit or to a documented `unknown` kind.

#### Scenario: SCIP emits a kind we recognize

- **WHEN** scip-dotnet emits a symbol with kind `Class`
- **THEN** the data model symbol's kind MUST be `class`

#### Scenario: SCIP emits a kind outside the closed set

- **WHEN** an indexer emits a kind we do not recognize
- **THEN** the data model MUST set kind to `unknown` and preserve the original kind string in a side metadata field for debugging

### Requirement: Occurrence representation

The data model SHALL represent each occurrence as `(canonical_path, symbol, range, role)` where `role` is one of: `definition`, `reference`, `read_access`, `write_access`, `import`. Multiple roles for the same `(path, symbol, range)` MUST be merged into a single entry whose role is the union (bitfield or multi-valued).

#### Scenario: A single occurrence has multiple roles

- **WHEN** an indexer emits an occurrence that is both a `definition` and a `write_access`
- **THEN** the stored entry MUST carry both roles, not be split into two rows

### Requirement: Relationship edges (explicit)

The data model SHALL represent indexer-declared relationships as edges with `kind ∈ {extends, implements, type_definition, override}` and `(from_symbol, to_symbol)` endpoints. These edges come directly from the indexer's relationship metadata and SHALL NOT require any derivation.

#### Scenario: Class implements interface

- **WHEN** scip-dotnet emits a `Relationship { is_implementation: true, symbol: "...IFoo#" }` on class `Bar`
- **THEN** the data model MUST contain an edge `(Bar, IFoo, implements)`

### Requirement: Derived dependency edges

The data model SHALL support deriving edges of `kind ∈ {calls, instantiates, field_type, property_type, param_type, return_type, local_var_type, attribute, throws, catches, imports_namespace, contains}` from occurrences combined with the enclosing-symbol declaration ranges. The derivation rule is: an occurrence at position P with role `reference` whose range is contained within the declaration range of an enclosing symbol S produces an edge `(S, occurrence.symbol, kind)` where `kind` is determined by the syntactic context recoverable from the indexer's data (e.g., position relative to declaration syntax). When `kind` cannot be determined, the edge MUST default to `references`.

#### Scenario: Field type reference

- **WHEN** a class `A` declares a field `Foo bar;` and the indexer emits an occurrence of `Foo` inside `A`'s declaration range at the field's type position
- **THEN** the data model MUST contain an edge `(A, Foo, field_type)`

#### Scenario: Method invocation inside a method body

- **WHEN** method `M` contains an occurrence of `OtherType.DoThing()` within `M`'s declaration range
- **THEN** the data model MUST contain an edge `(M, DoThing, calls)`

#### Scenario: Edge kind cannot be determined

- **WHEN** an occurrence is inside an enclosing symbol's range but its syntactic role cannot be derived from indexer data
- **THEN** the data model MUST emit an edge with `kind = references` rather than dropping the edge

### Requirement: External-package representation

The data model SHALL represent references to symbols from external packages with the package's name and version preserved on the symbol record. Indexer output that uses placeholder package descriptors (e.g., scip-dotnet's local `.` `.` form) MUST be normalized to a workspace-local sentinel package; cross-package references MUST retain their real `(package, version)`.

#### Scenario: Reference to System.String

- **WHEN** the indexer emits an occurrence of a symbol with descriptor `scip-dotnet nuget System.Runtime 8.0.0.0 System/String#`
- **THEN** the stored symbol record MUST have `package = "System.Runtime"`, `version = "8.0.0.0"` and `is_external = true`

#### Scenario: Reference to a workspace-local symbol

- **WHEN** the indexer emits a symbol with the local placeholder descriptor (e.g. `nuget . .`)
- **THEN** the stored symbol record MUST have `is_external = false` and `package` set to a workspace-local sentinel value (not literal `.`)

### Requirement: File-level dependency rollup

The data model SHALL produce file→file edges by aggregating occurrence-level references: for each `reference` occurrence in file F1 whose target symbol is defined in exactly one canonical_path F2 (F2 ≠ F1), emit an edge `(F1, F2)` with a multiplicity count equal to the number of such occurrences. Symbols defined in zero or multiple files MUST be excluded from this rollup. Local-scoped pseudo-symbols (e.g., SCIP `local N`) MUST be excluded.

#### Scenario: Reference to symbol with single defining file

- **WHEN** file `A.cs` contains 5 reference occurrences whose target symbol is defined exactly once at `B.cs`
- **THEN** the data model MUST contain a file edge `(A.cs, B.cs)` with `count = 5`

#### Scenario: Reference to symbol with multiple defining files (e.g., partial class)

- **WHEN** a target symbol is defined in 3 files via `partial class`
- **THEN** that occurrence MUST NOT contribute to file→file rollup (skipped, not split)

### Requirement: Project-level dependency rollup

The data model SHALL produce project→project edges by mapping each `canonical_path` to its project (per a workspace-configured path-prefix → project-name mapping) and aggregating file→file edges. Self-edges (project to itself) MUST be excluded.

#### Scenario: Cross-project reference

- **WHEN** files in `src/Web/` have file edges into files in `src/ApplicationCore/` totalling 121 references
- **THEN** the data model MUST contain a project edge `(Web, ApplicationCore)` with `count = 121`

### Requirement: Indexer-run provenance

Every record in the data model (symbol, occurrence, edge) SHALL carry the id of the indexer-run that produced it, including indexer name, version, source unit (e.g., `.sln` path), timestamp, and per-record success/partial status. This is required so a re-index of one source unit does not corrupt records produced by another.

#### Scenario: Re-indexing one .sln updates only its records

- **WHEN** a workspace has been indexed via `.sln A` and `.sln B` and we re-run only the indexer for `.sln A`
- **THEN** records previously associated with `.sln B` MUST remain unchanged
- **AND** records previously associated with `.sln A` MUST be replaced atomically with the new run's output

### Requirement: Edge kinds include `links_to` and `embeds`

The enumerated edge kinds SHALL include `links_to` (a reference from one node to
another) and `embeds` (transclusion — the source node inlines the target's
content), in addition to the existing code edge kinds. A `links_to` edge SHALL
be able to carry a match-quality grade (reusing the `match_kind` vocabulary) and
an optional relation. These additions SHALL NOT change the meaning of existing
code edge kinds.

#### Scenario: A graded markdown link is representable

- **WHEN** a markdown link resolves with a drifted match
- **THEN** a `links_to` edge is emitted carrying the drifted match-quality grade
- **AND** it round-trips through the store unchanged

#### Scenario: Transclusion uses the embeds kind

- **WHEN** a markdown node transcludes another via `![[…]]`
- **THEN** an `embeds` edge (distinct from `links_to`) is emitted

### Requirement: Markdown document and section identity

A markdown `document` and `section` symbol SHALL participate in the
`(canonical_path, symbol_string, range)` identity key using its `md:` native ID
as the `symbol_string` analog. The dedup/identity path SHALL accept a node whose
`symbol_string` is a markdown native ID rather than a code symbol string.

#### Scenario: Two sections in one file are distinct nodes

- **WHEN** a file has two headings producing native IDs `…#a` and `…#b`
- **THEN** they are retained as separate nodes keyed by their distinct
  `symbol_string` analogs at their respective ranges

#### Scenario: Document and its sections nest

- **WHEN** a file's `document` symbol contains heading sections
- **THEN** each section's `enclosing_sym_id` resolves to its parent section or
  the document symbol

