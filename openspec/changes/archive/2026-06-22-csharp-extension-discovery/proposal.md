## Why

A C# **extension method** — `static void Foo(this Order o)` declared in
`OrderExtensions` — is indexed today with exactly one structural edge:
`defined_in → OrderExtensions`. It has **zero** graph connection to `Order`, the
type it extends. As a direct consequence, listing the API surface of a type
(`list_in_scope(Order)`, "what can I call on an `Order`") **misses every
extension method**. In idiomatic C# — where LINQ, `Span` helpers, fluent
builders, and most library ergonomics are extension methods — that is a large,
silent hole in the type's surface.

Two grounding facts from the current code:

1. **Nothing ties an extension method to its receiver.** `EmitMethodRelationships`
   (`indexers/kenn-dotnet/src/Indexing/IndexerCore.cs`) emits only `Overrides`
   (base + interface). The wire `EdgeKind` enum
   (`indexers/kenn-dotnet/src/Wire/EdgeKind.cs`) has no extends/extension edge at
   all.
2. **Keying is correct and stays correct.** `PubId.ForMember` keys off
   `member.ContainingType`, so `Foo` is `OrderExtensions#Foo(Order)` — a static
   method of its holder, which is the truthful C# model. Calls already resolve:
   `order.Foo()` is walked through `ReducedFrom` back to the holder
   (`IndexerCore.cs:457`), so `calls`/callers are intact. **The gap is purely
   discovery *from the extended type*.**

A near-miss worth ruling out: walking `Foo`'s signature already emits a
`type_use` edge `Foo → Order` (the `this Order` parameter). But `Order`'s
incoming `type_use` is *every* method anywhere that takes an `Order` argument —
there is no way to distinguish the `this`-receiver from an ordinary parameter.
Too noisy to serve as the discovery edge; a dedicated edge is required.

## What Changes

Add one new edge kind, `extends_type`, source = the extension method, target =
the extended (receiver) type. The C# sidecar emits it at definition time; the
extended type's **incoming** `extends_type` edges are exactly its extension
methods. `defined_in` is untouched — the model never lies about where the method
lives.

- **Data model** — add `EdgeKind::ExtendsType` (`extends_type`), threaded through
  the wire enum (C#), `frames.ts`, the Rust wire parse, `kenn-model`'s `EdgeKind`
  (+ `as_str`), and the JSONL consumer's edge mapping. It parallels the existing
  `ExtendsRule` (CSS `@extend`) — kenn already models a non-containment "extends"
  relation.
- **C# producer** — in the member walk, when `method.IsExtensionMethod`, resolve
  the receiver type from `Parameters[0].Type` (its `OriginalDefinition` for
  generics like `this IEnumerable<T>`) and emit `ExtendsType(methodId,
  receiverTypeId)`.
- **MCP surface** — the edge is queryable the moment it exists:
  `find_usages(Order, edge_kinds=[extends_type])` / `list_usages` list a type's
  extension methods with no tool changes. A discoverability follow-up
  (`list_in_scope(... include_extensions)`) is noted as out of scope.

## Capabilities

### Modified Capabilities

- `source-data-model`: add the `extends_type` edge kind (extension/augmentation
  method → the type it extends); non-containment, parallels `extends_rule`.
- `dotnet-stream-indexer`: emit an `extends_type` edge for every C# extension
  method, from the method to its receiver type's `OriginalDefinition`; keep the
  existing `defined_in → holder` edge unchanged.
- `mcp-find-usages`: `extends_type` is an addressable incoming edge kind, so a
  type's extension methods are reachable via `find_usages` / `list_usages`.

## Impact

- **Independent change.** Touches the C# producer + the shared edge enum only.
  Not blocked on, nor blocking, `add-swift-index` (Swift keys extension members
  to the extended type natively and needs no `extends_type` — see that change's
  design).
- **Backward compatible.** A new edge kind is additive; existing nodes/edges and
  pub_ids are unchanged. Re-index required to populate the new edges.
- **Surface:** one wire enum entry, one Rust enum entry, `frames.ts`, one
  emission site in `IndexerCore.cs`, the consumer edge map. No new node kinds, no
  pub_id changes, no MCP tool signature changes.
- **Generality:** `extends_type` is named for the concept, not C# — Kotlin
  extension functions (a future producer) would reuse it. Swift does not.
