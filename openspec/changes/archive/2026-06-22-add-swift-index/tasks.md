# Tasks

Phased: Phase 0 lands the Rust plumbing (Swift becomes a known, enable-able
language with a no-op-until-present driver); Phases 1–4 build the sidecar from
store-provisioning → symbols → edges → extension/kind semantics; Phase 5
packages and verifies end-to-end on a real SwiftPM package. Sidecar reader:
`swift-index-store` (design D2). Scope: SwiftPM only (D-non-goals).

## Phase 0 — Rust plumbing (mirror `KennDotnet`)

- [x] 0.1 Add `Language::Swift` to `crates/kenn-model/src/language.rs`: prefix `sw`, extension `swift`, project file `Package.swift`, `db_name` `swift`, `from_prefix`/`from_db_name`/test arrays. Also the two extra exhaustive match sites: `short_id.rs` partition (`9`) and `transform/naming.rs` test-descriptor (`false`, like C#). → verify: `cargo test -p kenn-model` (46 pass) — prefix/db_name round-trip; `.swift` maps to `Swift`.
- [x] 0.2 `transform/lang.rs`: `"swift"`/`"Swift" => Language::Swift` in `language_from_scip` + `"swift"` in `language_from_path`; `transformer_for` returns `None` for Swift (JSONL path). → verify: tests pass — `language_from_path("…​.swift")` is `Some(Swift)`; `transformer_for(Swift)` is `None`.
- [x] 0.3 `crates/kenn-config/src/language/swift.rs` (`SwiftConfig`: `enabled`/`command`/`projects`/`skip_build`/`excludes`), field on `LanguageConfig`, crate re-export, and `validate()` + disjoint-excludes guard updates. → verify: kenn-config tests pass; `indexing_signature` serializes the whole `LanguageConfig` so Swift is auto-included.
- [x] 0.4 `crates/kenn-indexer/src/driver/swift.rs` (`KennSwift: JsonlIndexer`, discovers `Package.swift`), registered in `driver/mod.rs` + `configure_runner` (jsonl driver when `swift.enabled`). → verify: mirrors `KennDotnet` — `run` returns `JsonlOutcome::Unavailable` on a missing binary (NotFound), not a crash.

## Phase 1 — sidecar skeleton + store provisioning

- [x] 1.1 Created `indexers/kenn-swift/` (SwiftPM package, `swift-index-store` dep pinned by branch — it carries unsafe flags) with CLI `index --workspace --projects --skip-build`. → verify: `swift build` produces the binary; runs against a fixture.
- [x] 1.2 `Package.swift` discovery (`Provisioning.discoverPackages`, skips `.build`/`.git`). → verify: `kenn_swift_resolves_package_manifests` (Rust driver test) finds nested manifests.
- [x] 1.3 Store provisioning: `swift build --build-tests` (incremental) then locate `.build/<triple>/<config>/index/store`; `--skip-build` reads only, clear error if absent (D1). → verify: fixture builds then finds a non-empty store; the real store path was `.build/arm64-apple-macosx/debug/index/store`.
- [x] 1.4 Wire envelope: `MetaFrame(language: "swift")` + `EndFrame`. → verify: real kenn index ingested it — store shows `language=swift`, counts match the sidecar (6 docs / 27 syms / 25 edges).

## Phase 2 — files & symbols

- [x] 2.1 `FileFrame` per source file (workspace-relative path, FNV-1a `content_hash`). → verify: fixture `Sources/SampleApp/*.swift` appear as file frames.
- [x] 2.2 `SymbolFrame` per definition with composed readable `key` (D7), name, projected kind, resolved parent, file, 0-based def range; `sig` = label-bearing name for callables; overloads salted (`==(_:_:)#6d1ef5`). → verify: `EndToEndTests` + store shows `sw:SampleApp.Order`, `sw:SampleApp.Order.save()`.
- [x] 2.3 Kind projection (D4) in `KindMap.wireKind`: protocol→interface, actor/class→class, struct/enum, method/function, init→constructor, property/field, enum_member, typealias→type; accessors suppressed. → verify: store kinds (`interface`, `struct`, `class`, `method`, `constructor`, `property`).
- [x] 2.4 Test-target sources flagged `test:true` (by `Tests/` path; sidecar runs `--build-tests`). → verify: `sw:SampleAppTests.OrderTests*` symbols are `test:true`.

## Phase 3 — edges

- [x] 3.1 `calls` from the `calledBy` relation. → verify (store): `sw:SampleApp.Cart.checkout() --calls--> sw:SampleApp.Order.save()`, plus a test→code call.
- [x] 3.2 `implements` from `baseOf` (conformance + inheritance; direction corrected via fixture — `related implements occ.symbol`). → verify: `Order --implements--> Persistable`, `Derived --implements--> Base`.
- [x] 3.3 `overrides` from `overrideOf`. → verify: `Derived.run() --overrides--> Base.run()`; protocol-requirement satisfaction also surfaces as `overrides` (`Order.save() --overrides--> Persistable.save()`).
- [x] 3.4 `imports` module-dependency edges — done (during `add-swift-xcode`). A module reference (`import Foo`, kind=`module`, non-def) records `currentModule imports Foo`; emitted as `imports` edges between synthetic `module`-kind nodes keyed `sw:<name>` (modules have no in-source definition). → verify: Ice → SwiftUI/Combine/Cocoa/Foundation/AXSwift/… (20 edges), 0 unresolved.

## Phase 4 — extension & partial semantics

- [x] 4.1 Extension members key to the extended type. Swift gives them a `childOf` of the *extension* (`s:e:…`, which carries no relations), so the extended type is recovered by longest-prefix-matching that USR against defined nominal types (`resolveParent`). → verify: `sw:SampleApp.Order.describe()` (declared in `OrderExt.swift`) parents to the canonical `SampleApp.Order`, located in `OrderExt.swift`.
- [x] 4.2 Cross-file extension collapses onto one node. → verify: a single `SampleApp.Order` node carries `describe()` (from `OrderExt.swift`) alongside `save()`/`total()` (from `Order.swift`); the duplicate `Order` stub is gone.
- [x] 4.3 No `extends_type` edge for Swift. → verify: extension members appear via membership/parent, not an augmentation edge.

## Phase 5 — packaging & end-to-end verification

- [x] 5.1 Release build + `just build-indexer-swift` → `build/kenn-swift` (302K), `dlopen`s libIndexStore from the toolchain; README documents the runtime dependency. → verify: release binary indexes the fixture.
- [x] 5.2 End-to-end through the real kenn binary: `[language.swift]` enabled, `kenn index` on the fixture → SQLite store carries `sw:` pub_ids and the resolved `calls`/`implements`/`overrides` graph (across the SampleApp + SampleAppTests modules). Also the in-package `EndToEndTests` (Swift) builds the fixture, reads the store, and asserts the same. → verify: store query (above) + `just test-indexer-swift` green.
- [x] 5.3 Quality gates: clippy clean; CRAP gate PASSED (added `driver/tests.rs` coverage for `KennSwift` so `swift.rs` is under threshold); `cargo fmt --all` (touched only edited files); `dotnet format` N/A; `swift-format` not installed (no project rule mandates it). → verify: all green.
- [x] 5.4 Real-repo run on `apple/swift-argument-parser` (161 files): **4984 symbols, 6819 edges, 1012 external stubs, 0 unresolved edge targets**; full `kenn index` ingests it (store counts match) in ~3s warm. Exposed and fixed a **stack overflow** (`keyFor` recursed on a parent cycle the prefix-resolution can create — added a visiting-set guard + `resolved != usr` check). Verified readable keys (no leaked USRs), overload salting (812 keys), nested types, and real conformance/call/override semantics.
