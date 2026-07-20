## ADDED Requirements

### Requirement: Enum members are emitted as `enum_member`

The C# producer SHALL emit `enum_member` as the wire `SymbolKind` for a field whose containing type is an enum, rather than `const`. This adopts the shared wire's `enum_member` value so the consumer resolves it to `Kind::EnumMember`, matching the Rust and Go indexers. Non-enum constant fields SHALL continue to emit `const`.

#### Scenario: C# enum member classifies as enum_member

- **WHEN** the producer walks a member of a C# `enum`
- **THEN** its `SymbolFrame.kind` is `enum_member` (not `const`), and the consumer resolves it to `Kind::EnumMember`

#### Scenario: A non-enum const field is unchanged

- **WHEN** the producer walks a `const` field that is not an enum member
- **THEN** its `SymbolFrame.kind` remains `const` → `Kind::Constant`
