## Why

kenn indexes C#, TypeScript, Rust, Python, and the markup/stylesheet family — but
not Swift. Swift is a major application and server language with no kenn coverage,
so Swift packages are invisible to symbol search, navigation, and findings.

Swift deliberately sits outside the SCIP ecosystem the SCIP-family producers
(rust-analyzer, scip-python) ride: the canonical SCIP indexer registry
(scip-code.org) lists C#, C/C++, Dart, Go, Java/Scala/Kotlin, PHP, Python, Ruby,
Rust, TS/JS — **Swift is absent**, and there is no maintained `scip-swift`.
Instead, Swift uses Apple's **index store** ("indexing while building"): the
compiler emits a libIndexStore unit/record database as a byproduct of a normal
build, carrying fully-resolved symbols (USRs), occurrences, and typed relations
(`calledBy`, `conformsTo`, `overrides`). That maps almost 1:1 onto kenn's graph.

This makes Swift a **JSONL-sidecar** language, not a SCIP one — the exact shape
of `kenn-dotnet`: a per-language binary that reads a semantic model and streams
`frames.ts` JSONL, which the existing consumer assembles into the graph.

```
  kenn-dotnet:  .csproj ─Roslyn─▶ semantic model ─▶ kenn JSONL frames
  kenn-swift:   swift build ─▶ .build/index/store ─(swift-index-store)─▶ kenn JSONL frames
                                              (no SCIP anywhere)
```

## What Changes

Add Swift as a first-class indexed language via a new Swift sidecar
(`indexers/kenn-swift/`) that builds the package, reads its index store, and emits
kenn JSONL. The Rust side is minimal because the JSONL pipeline assembles pub_ids
generically (`pub_id = format!("{}:{}", language.prefix(), key)`,
`transform_jsonl/stream.rs`) — Swift needs **no** `kenn-model` id transformer and
**no** SCIP transformer (exactly like C#).

- **Sidecar** (the bulk) — a Swift binary using `swift-index-store`
  (libIndexStore wrapper). It (a) ensures the index store exists — read
  `.build/index/store` if fresh, else run `swift build` (a `--skip-build` escape,
  mirroring kenn-dotnet's `--skip-restore`); (b) walks units/records; (c) emits a
  `MetaFrame(language: "swift")`, `FileFrame`s, `SymbolFrame`s per definition, and
  `EdgeFrame`s per relation (`calls`/`implements`/`overrides`/`imports`).
- **Plumbing** (mechanical, mirrors `KennDotnet`) — `Language::Swift` (prefix
  `sw`, extension `swift`, project file `Package.swift`); two `transform/lang.rs`
  arms (`"swift"`→`Swift`); a `SwiftConfig` (clone of `CsharpConfig`); a
  `KennSwift: JsonlIndexer` driver (clone of `KennDotnet`); one registration in
  `configure_runner`.
- **Extension semantics** — Swift `extension Foo { … }` members key to the
  **extended type** `Foo` (their natural index-store parent), reusing the existing
  `partial` collapse path — so `list_in_scope(Foo)` includes them with no new
  machinery. Retroactive conformance (`extension Foo: Bar {}`) emits `implements`.
- **Scope** — SwiftPM only (`Package.swift`, `.build/index/store`). Xcode
  (`.xcodeproj`/DerivedData) is explicitly deferred to a follow-up.

## Capabilities

### New Capabilities

- `swift-stream-indexer`: the Swift sidecar — `Package.swift` discovery, index
  store provisioning (read-fresh or `swift build`), libIndexStore unit/record
  traversal via `swift-index-store`, and emission of the `frames.ts` wire
  (`MetaFrame(language: "swift")`, files, symbols with per-site def ranges,
  `calls`/`implements`/`overrides`/`imports` edges). Extension members key to the
  extended type; the extension block reuses the `partial` additional-def
  mechanism.

### Modified Capabilities

- `source-data-model`: add the `sw:` language prefix (extension `.swift`, project
  file `Package.swift`); Swift symbols flow through existing node and edge kinds —
  no new kinds (protocol→`interface`, actor→`class`, subscript→`property`/`method`
  chosen in the sidecar; `extension` reuses `partial`).
- `indexing-orchestrator`: register the Swift JSONL driver in `configure_runner`
  when `[language.swift]` is enabled, as a sibling producer alongside C#/TS.
- `jsonl-indexer-driver`: Swift reuses the existing `JsonlIndexer` contract
  unchanged; the consumer's language is already parameterized off
  `MetaFrame.language` (`stream.rs` `on_meta`), so no consumer change beyond the
  `"swift"` language mapping.

## Impact

- **New sidecar:** `indexers/kenn-swift/` (a SwiftPM package). New build dependency
  — a **Swift toolchain** to compile the sidecar and to produce the index store
  of target packages. Consistent with kenn-dotnet requiring a .NET SDK at runtime;
  a Swift project being indexed already has a Swift toolchain.
- **The one genuinely new wrinkle — the build dependency.** Unlike rust-analyzer
  (self-contained analysis), the index store only exists if the package was
  compiled with indexing on. SwiftPM emits it automatically under
  `.build/index/store` during a debug `swift build`; the sidecar triggers that
  build when the store is stale/absent. Broken builds yield a partial store (and a
  clear error), not a crash.
- **Platforms:** macOS + Linux (open-source Swift toolchain ships libIndexStore).
  Xcode-only projects are out of scope this change.
- **Rust surface:** one `Language` variant, two `transform/lang.rs` arms, a
  `SwiftConfig`, a `KennSwift` driver, one `configure_runner` block. No id
  transformer, no kind classifier arm, no SCIP transformer (JSONL path).
- **Distribution:** a prebuilt Swift binary per platform (`swift build -c
  release`), `dlopen`-ing `libIndexStore` from the toolchain at runtime.
