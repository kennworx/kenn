## Context

Swift's semantic index is the **index store**, not SCIP. The compiler, given
`-index-store-path` (set automatically by SwiftPM during a debug build), emits a
libIndexStore unit/record database under `<pkg>/.build/index/store`:

```
  swift build ─▶ .build/index/store/v5/{units,records}/
                   │
                   ├ unit  = one compiled file/module: its deps + record pointer
                   └ record = per-file symbol occurrences (def/ref) + relations
                              (USR-keyed: calledBy, conformsTo, overrides, childOf)
```

The mapping to kenn is near-mechanical:

```
  index store            kenn
  ─────────────────────────────────────
  USR                  → key (consumer stamps sw:USR-descriptor)
  occurrence (def)     → SymbolFrame + def range
  relation calledBy    → calls edge
  relation conformsTo  → implements edge
  relation overrides   → overrides edge
  unit dependency      → imports edge
```

So the sidecar is **read-and-translate**, not analyze — the kenn-dotnet pattern
with libIndexStore where Roslyn sits. The Rust pipeline already assembles pub_ids
generically (`pub_id_for(key) = "{prefix}:{key}"`, `transform_jsonl/stream.rs`),
and the consumer's language is set from `MetaFrame.language` via `on_meta`, so the
only Rust additions are the `Language` variant and its prefix/extension mapping.

Investigation trail (no off-the-shelf path exists): scip-code.org lists no Swift
indexer; sourcekit-lsp uses IndexStoreDB, not SCIP; the `scip` CLI cannot ingest
an index store (its conversions are SCIP→SQLite / SCIP→LSIF); no Rust crate wraps
libIndexStore. The mature readers are Swift — `MobileNativeFoundation/swift-index-store`
and `kateinoigakukun/swift-indexstore`. Hence: a Swift sidecar (lane A), not SCIP
(no emitter) and not a Rust FFI in-process reader (would be the first to bind
libIndexStore).

## Goals / Non-Goals

**Goals:**

- Swift as a first-class semantic language: symbols, defs, and the
  `calls`/`implements`/`overrides`/`imports` graph, on par with C#.
- Reuse the JSONL pipeline wholesale; keep the Rust delta to plumbing.
- SwiftPM workflow that "just works": discover `Package.swift`, ensure the store,
  read it, emit frames.

**Non-Goals:**

- Xcode / `.xcodeproj` / `.xcworkspace` / DerivedData (hashed global store path,
  scheme/SDK/signing config, macOS-only) — a separate follow-up change.
- A Rust in-process libIndexStore reader (lane B) — only if "no sidecar process"
  ever becomes a hard requirement.
- SCIP as an intermediate format (lane C, `lsp-to-scip` + sourcekit-lsp) — lower
  fidelity than the raw relation graph; not pursued.
- New node or edge kinds. Swift constructs project onto the existing wire
  `SymbolKind`/`EdgeKind` sets in the sidecar.

## Decisions

### D1 — Build trigger: `swift build`, with a read-fresh fast path

SwiftPM emits the index store automatically during a debug `swift build` (no
special flag). The sidecar: if `.build/index/store` exists and is fresh, read it
directly (fast path); otherwise run `swift build` to produce it, then read. A
`--skip-build` flag forces read-only (mirrors kenn-dotnet's `--skip-restore`).

Rejected — driving `swiftc -index-store-path` per target (lane "c"): reconstructing
each compiler invocation (search paths, module maps, dependency `.swiftmodule`s)
reimplements the build system the moment a package has dependencies. Delegate to
SwiftPM, exactly as kenn-dotnet delegates to MSBuild.

### D2 — Reader: `swift-index-store` (libIndexStore wrapper)

Use `MobileNativeFoundation/swift-index-store` as the SwiftPM dependency for
unit/record traversal; it wraps the first-party `libIndexStore` (shipped in the
toolchain, `dlopen`-ed at runtime). No raw unit/record parsing, no FFI we own.

### D3 — Swift `extension` keys to the extended type (the `partial` path)

A Swift `extension Foo { func bar() }` adds `bar` as a member of `Foo` — its
index-store parent is `Foo`. The sidecar emits `bar` as a member keyed to `Foo`
(`Foo#bar()`), carrying the extension file's own def range; cross-file extension
members "just work" through ordinary member emission. If the extension block's own
location is worth recording, emit `Foo` as an additional `partial: true` def site
(the existing consumer collapse, `transform_jsonl/stream.rs`). This is the
**opposite** of C#: a C# extension method keys to its *holder* and needs a
separate `extends_type` edge (see `csharp-extension-discovery`); Swift needs none.

Retroactive conformance `extension Foo: Bar {}` emits `implements` (`Foo`→`Bar`)
regardless of where declared — the index store records the `conformsTo` relation.

### D4 — Kind projection onto the existing wire `SymbolKind`

The sidecar maps Swift constructs to the fixed wire `SymbolKind` union:
`protocol`→`interface`, `actor`→`class`, `struct`/`enum`/`class`→themselves,
`func`/`method`→`function`/`method`, `var`/`let` member→`property`/`field`,
`subscript`→`property` (or `method`), `case`→`enum_member`, `typealias`→`type`,
`init`→`constructor`, `deinit`→`destructor`, `operator`→`function`. No new kind is
added; `associatedtype` maps to `type`. Decisions live in the sidecar's KindMap.

### D5 — Prefix `sw`, `MetaFrame.language: "swift"`

Two-letter prefix `sw`, consistent with `cs`/`ts`/`rs`/`go`/`py`. The sidecar
declares `language: "swift"` once in `MetaFrame`; the consumer maps `"swift"`→
`Language::Swift` (`transform/lang.rs` `language_from_scip`) and stamps `sw:`. The
sidecar emits prefix-free, language-naked keys — no id transformer needed.

### D6 — Packaging: prebuilt per-platform binary

Ship `kenn-swift` as a self-contained `swift build -c release` binary per platform
(macOS arm64/x64, linux x64/arm64), `dlopen`-ing `libIndexStore` from the
toolchain at runtime. A package being indexed already has a Swift toolchain (it
produced the store), so the runtime dependency is satisfied by construction —
parallel to kenn-dotnet needing a .NET SDK present.

### D7 — Wire key: readable composed name, full signature in `sig`

The wire `key` is a readable, C#-style qualified name composed from the symbol's
`name` + its `childOf` parent chain (module → type → member), e.g.
`MyApp.Order.save` → pub_id `sw:MyApp.Order.save`. It is NOT the raw mangled USR
and does NOT carry parameter types — the full rendered Swift signature
(`save(x: Int) -> Bool`) lives in the separate `sig` field (SymbolFrame.sig →
SymbolDocsRecord.sig → surfaced by `get_symbol`/search), exactly as kenn-dotnet
populates `sig` via `BuildSignatureDoc`. `pub_id` and signature are separate
concerns per the wire contract; duplicating param types into the key buys
nothing. The key only needs to be stable and unique: overloads (the one thing
that collides a composed name) are disambiguated with a short USR-tail salt
appended only on actual collision. (Resolved from the prior open question — the
signature being captured separately settles it toward readability.)

## Open Questions

- **Store staleness criterion:** how "fresh" is judged for the fast path —
  store mtime vs source mtime, or always rebuild when any `.swift` is newer than
  the store. Lean: rebuild if any source is newer than the unit timestamps;
  `--skip-build` overrides.
- **Module granularity / packages:** multi-target packages and local
  dependencies — one `swift build` covers the whole package graph; confirm the
  store spans all local targets and that external dependency symbols are emitted
  as external stubs, not first-class.
- **Test target handling:** map `.testTarget` sources to `is_test` (the wire
  `Test` flag) — likely by target kind or path, mirroring kenn-dotnet's
  `IsTest`.
- **subscript/operator kind:** `subscript` as `property` vs `method`; revisit
  once real-repo navigation is observed.
- **USR → key shape:** how much of the USR to keep vs re-render into C#-style
  `Module.Type#member(params)` for readable pub_ids; a spike on a real package
  decides legibility vs effort.
