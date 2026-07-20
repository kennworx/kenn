# Tasks

One new edge kind threaded producer → wire → consumer, then emitted for C#
extension methods, then verified end-to-end. Phase 1 (the kind) gates Phase 2
(emit) and Phase 3 (ingest); Phase 4 is the cross-layer verification.

## Phase 1 — the `extends_type` edge kind

- [x] 1.1 Add `ExtendsType` to the wire enum `indexers/kenn-dotnet/src/Wire/EdgeKind.cs` and its `ToWireString`/parse (`extends_type`). → verify: `dotnet test indexers/kenn-dotnet.tests` round-trips the wire string.
- [x] 1.2 Add `extends_type` to `indexers/frames.ts` (`EdgeKind` union / docs). → verify: the string matches the C# and Rust spellings exactly.
- [x] 1.3 Add `EdgeKind::ExtendsType` + `as_str` `"extends_type"` to `crates/kenn-model/src/edge.rs`. → verify: `cargo test -p kenn-model` round-trips `as_str`/parse. Also threaded the variant through `kenn-store` `codes.rs` (on-disk code `16`, `ALL_EDGE_KINDS`) and the SCIP-occurrence classifier `kenn-indexer/src/edge.rs` (producer-emitted → `unreachable!` group).
- [x] 1.4 Map the wire edge → `EdgeKind::ExtendsType` in the JSONL consumer (`crates/kenn-indexer/src/transform_jsonl/records.rs`). → verify: a hand-built JSONL fixture with an `extends_type` edge ingests to a model edge of that kind.

## Phase 2 — emit it from the C# producer

- [x] 2.1 In the member walk (`IndexerCore.cs`, in `EmitMethodRelationships`), when `method.IsExtensionMethod`, resolve `method.Parameters[0].Type.OriginalDefinition` and emit `EmitEdge(ExtendsType, methodId, EnsureRefStub(receiverType))`. → verify: `ExtensionMethodEmitsExtendsTypeToReceiver` — `Touch(this Order)` yields one `extends_type` edge to `Shop.Order`.
- [x] 2.2 Generic receiver: `static T FirstOr<T>(this IEnumerable<T>)` targets the `IEnumerable<>` `OriginalDefinition`. → verify: same test asserts the edge target key contains `IEnumerable` (open generic).
- [x] 2.3 Receiver type defined in another assembly emits a ref-stub target (external), not a dropped edge. → verify: the `IEnumerable<T>` receiver (external) resolves to a stub-keyed target, edge present.

## Phase 3 — consumer / graph wiring

- [x] 3.1 Confirm the ingested `extends_type` edge is stored and addressable. → verify: `edge_kind_codes_are_unique_and_round_trip` (`kenn-store/src/db/codes.rs`) — `extends_type` has stable code `16` round-tripping to its relation name; the store's incoming/outgoing edge storage is generic over this code.
- [x] 3.2 Confirm `find_usages` / `list_usages` accept `extends_type` in `edge_kinds`. → verify: by construction — `edge_kinds: Option<Vec<EdgeKind>>` (`kenn-mcp/src/tools/query.rs:639,855`) deserializes `"extends_type"` straight from the serde/schemars `EdgeKind` enum; no allowlist change. Correctly absent from the default reference set (design D5, opt-in).

## Phase 4 — end-to-end verification

- [x] 4.1 Fixture C# (in `ExtensionMethodEdgeTests`): `Order` + `OrderExtensions` (two extension methods) + `Other.Plain(Order)`. → verify: exactly two `extends_type` edges; `Plain` produces none.
- [x] 4.2 Regression: the extension method keeps `defined_in → OrderExtensions`. → verify: `ExtensionMethodKeepsDefinedInHolder` asserts `Touch`'s `defined_in` target is the holder static class.
- [x] 4.3 `dotnet format` the two .NET projects (per CLAUDE.md §8) and `cargo clippy --workspace --all-targets` + `just crap-ci` clean for the Rust side (§5–§7). → verify: all green — clippy clean, CRAP gate PASSED (no regressions/new over-threshold), `dotnet format` no churn, 26/26 dotnet tests pass; `cargo fmt --all` last (touched only the 4 edited files).
