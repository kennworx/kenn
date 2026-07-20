## ADDED Requirements

### Requirement: The Swift sidecar discovers SwiftPM packages and provisions an index store

The Swift indexer SHALL discover `Package.swift` packages under the workspace and,
for each, ensure a libIndexStore index store exists before reading. If a fresh
`.build/index/store` exists it SHALL be read directly; otherwise the indexer SHALL
run `swift build` (which emits the store as a debug-build byproduct) and then read
it. A `--skip-build` flag SHALL force read-only behavior and SHALL report a clear
error when no store is present. SwiftPM is the only supported build system this
change; Xcode projects are out of scope.

#### Scenario: a never-built package is built then indexed

- **WHEN** the sidecar indexes a `Package.swift` package with no `.build/index/store`
- **THEN** it runs `swift build` and reads the resulting index store

#### Scenario: a fresh store is read without rebuilding

- **WHEN** `.build/index/store` is up to date relative to sources
- **THEN** the sidecar reads it directly without invoking `swift build`

#### Scenario: skip-build with no store errors clearly

- **WHEN** `--skip-build` is set and no index store exists
- **THEN** the sidecar exits with a clear "no index store; run `swift build`" error
  rather than producing an empty index silently

### Requirement: The Swift sidecar emits the kenn JSONL wire for Swift symbols and relations

The indexer SHALL stream `frames.ts` JSONL: a single `MetaFrame` declaring
`language: "swift"`, a `FileFrame` per source file with a workspace-relative path,
a `SymbolFrame` per definition (USR-derived language-naked `key`, name, projected
kind, parent, file, def range), and `EdgeFrame`s for the index store's relations —
`calls` (call relations), `implements` (`conformsTo`, including retroactive
conformance), `overrides`, and `imports` (module dependencies). Keys SHALL be
prefix-free; the consumer stamps `sw:`.

#### Scenario: a struct and its method become symbols

- **WHEN** the store contains `struct Order { func save() }`
- **THEN** the wire carries a `SymbolFrame` for `Order` and one for its `save`
  method, each with its definition range

#### Scenario: a protocol conformance becomes an implements edge

- **WHEN** `struct Order: Persistable {}` is indexed
- **THEN** an `implements` edge is emitted from `Order` to `Persistable`

#### Scenario: the language is declared once

- **WHEN** the sidecar produces output
- **THEN** exactly one `MetaFrame` is emitted with `language` equal to `"swift"`,
  and no per-symbol language prefix appears on the wire

### Requirement: Swift extensions key members to the extended type

Members declared in a Swift `extension Foo { … }` SHALL be emitted as members of
the extended type `Foo` (keyed to `Foo`), carrying the extension file's own
definition range, so the extended type's member listing includes them across
files. No augmentation edge (`extends_type`) SHALL be emitted for Swift — extension
members are surfaced through membership. A retroactive conformance declared in an
extension (`extension Foo: Bar {}`) SHALL still emit an `implements` edge.

#### Scenario: an extension method appears on the extended type

- **WHEN** `extension Order { func total() {} }` is declared in `Order+Total.swift`
- **THEN** `total` is a member of `Order`, located in `Order+Total.swift`

#### Scenario: extension members across files collapse onto one type

- **WHEN** `Order` has members in `Order.swift` and an `extension` in another file
- **THEN** the graph has a single `Order` node carrying members from both files
