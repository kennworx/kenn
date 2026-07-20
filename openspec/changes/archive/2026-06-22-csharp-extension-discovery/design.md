## Context

C# extension methods are static methods that *appear* as instance members at the
call site. Roslyn — and therefore kenn — models them truthfully as members of
their **holder** static class:

```
  static class OrderExtensions {            defined_in
      static void Foo(this Order o) { } ───────────────▶ OrderExtensions   ✓ today
  }                                    ╴╴╴╴╴╴╴╴╴╴╴╴╴╴╴╴╴▶ Order             ✗ missing
                                          extends_type (NEW)

  order.Foo();   ──calls──▶ OrderExtensions#Foo(Order)   ✓ today (via ReducedFrom)
```

`PubId.ForMember` keys `Foo` off `member.ContainingType` (the holder), and
`EnsureRefStub` walks a reduced call (`order.Foo()`) back through `ReducedFrom`
to the holder declaration. Both are correct and stay. What's absent is any edge
from `Order` to the methods that extend it, so the type's surface — as seen by
`list_in_scope` / `find_usages` on `Order` — is incomplete.

This mirrors a relation kenn already has for stylesheets: `extends_rule` (CSS
`@extend` / Sass `composes`) is a non-containment "this rule extends that rule"
edge. `extends_type` is the code-symbol analogue.

## Goals / Non-Goals

**Goals:**

- A type's extension methods are reachable *from the type*, via a single new
  incoming edge kind, without altering `defined_in` (no lie about where the
  method is declared).
- C#-producer-only change riding the existing JSONL pipeline; no new node kind,
  no pub_id change.

**Non-Goals:**

- Folding extension methods into the type's member list as if declared there
  (would corrupt `defined_in` / `get_symbol(...).parent`). Rejected — see
  Alternatives.
- New MCP tools or a `list_in_scope` union flag. The edge is queryable via
  existing `find_usages`/`list_usages`; a discoverability flag is a later,
  optional change.
- Swift / other languages. Swift extension members key to the extended type
  natively (see `add-swift-index`); this edge is driven by C#'s holder model.

## Decisions

### D1 — One new edge kind `extends_type`, method → type

Source = the extension method symbol; target = the extended (receiver) type.
Direction matches `implements` (concrete → interface): the *augmenting* symbol
points at the *augmented* one. A type's extension methods are then its **incoming**
`extends_type` edges — the shape `find_usages`/`list_usages` already return.

Rejected: holder-class → type (coarse; you want individual methods, not "this
static class extends something").

### D2 — Receiver type via `Parameters[0].Type.OriginalDefinition`

For `IsExtensionMethod` members, the extended type is the first (`this`)
parameter's type. Target its `OriginalDefinition` so `static void Foo<T>(this
IEnumerable<T>)` attaches to the generic `IEnumerable<>` node, consistent with
how other edges normalize to `OriginalDefinition`. Emit at definition (walking
the holder's members), not at call sites — one edge per extension method,
independent of call count.

### D3 — Thread the kind through every layer; no shortcut

The wire `EdgeKind` (C#), `frames.ts`, the Rust wire-frame parse, `kenn-model`'s
`EdgeKind` + `as_str`, and the JSONL consumer's wire→model edge map all enumerate
edge kinds explicitly. `extends_type` is added to each. This is the same spread
every existing edge kind already pays; there is no central table to amend
instead.

### D4 — `defined_in` stays; the two edges coexist

`Foo` keeps `defined_in → OrderExtensions` and gains `extends_type → Order`. A
consumer reading the member list of `OrderExtensions` still sees `Foo`; a
consumer reading `Order`'s extension surface follows incoming `extends_type`.
Neither view is polluted.

### D5 — MCP exposure is free; surface it in the default-discoverable set

Once the edge exists, `find_usages(Order, edge_kinds=[extends_type])` and
`list_usages` return it with no code change. Open question (below): whether
`extends_type` joins the *default* `find_usages` reference set or stays opt-in
via `edge_kinds`. Lean opt-in initially, to avoid mixing "methods that extend
this type" into "references to this type" unannounced.

## Alternatives considered

- **Second `defined_in` (Foo → Order too).** Cheapest — `list_in_scope` would
  surface it with no query change — but it makes `Foo` appear declared in two
  places, corrupts `get_symbol(Foo).parent`, and double-counts in the holder's
  own member list. Rejected.
- **Reuse `type_use`.** Already emitted for the `this` param, but
  indistinguishable from any other `Order`-typed parameter at the `Order` end.
  Too noisy. Rejected.
- **Synthesize the union only in the MCP layer (no edge).** Would require the
  query layer to re-derive "is this a `this`-param" from signatures at read time;
  the producer already knows `IsExtensionMethod` for free. Push it to index time.

## Open Questions

- **Default edge set:** does `extends_type` join `find_usages`' default
  reference edges, or stay opt-in? (D5 — lean opt-in.)
- **`list_in_scope` union:** a follow-up `include_extensions` flag that merges
  incoming `extends_type` into a type's member listing — in scope for a later
  change, not this one.
- **Other extension-shaped C# constructs:** C# 14 extension blocks /
  extension members, if/when the pinned Roslyn (4.7) is bumped, may surface the
  same relation through a different symbol shape. Out of scope at Roslyn 4.7.
