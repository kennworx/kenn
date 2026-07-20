## ADDED Requirements

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
