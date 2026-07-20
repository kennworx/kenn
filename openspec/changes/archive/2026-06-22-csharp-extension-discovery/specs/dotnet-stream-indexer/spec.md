## ADDED Requirements

### Requirement: The C# indexer emits `extends_type` for extension methods

The C# indexer SHALL emit an `extends_type` edge for every extension method (a
method where `IsExtensionMethod` holds), from the method to the type it extends.
The extended type SHALL be resolved from the method's first (`this`) parameter
type, normalized to its `OriginalDefinition` so a generic receiver
(`this IEnumerable<T>`) targets the open generic type. The method's existing
`defined_in` edge to its holder static class SHALL be unchanged, and call
resolution (`order.Foo()` → the holder declaration via `ReducedFrom`) SHALL be
unaffected.

#### Scenario: a simple extension method

- **WHEN** the indexer walks `static void Foo(this Order o)` in `OrderExtensions`
- **THEN** it emits `extends_type` from `Foo` to `Order`
- **AND** it still emits `defined_in` from `Foo` to `OrderExtensions`

#### Scenario: a generic receiver targets the open type

- **WHEN** the indexer walks `static T First<T>(this IEnumerable<T> xs)`
- **THEN** the `extends_type` target is the `IEnumerable<>` original definition,
  not a constructed `IEnumerable<SomeConcrete>`

#### Scenario: an ordinary parameter does not create the edge

- **WHEN** a non-extension method takes an `Order` parameter
- **THEN** no `extends_type` edge to `Order` is emitted (only the existing
  `type_use` for the parameter type)

#### Scenario: a receiver type from another assembly

- **WHEN** an extension method extends an external type (`this string`)
- **THEN** an `extends_type` edge is emitted to an external stub for that type,
  not dropped
