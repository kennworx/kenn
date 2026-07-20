# Tasks

Reuses `Indexer.swift` verbatim — all work is in `Provisioning`/CLI plus
verification. The validated recipe (spike on Ice + Food Truck) is the spec.

## Phase 0 — read primitive (landed in the spike)

- [x] 0.1 `--store <path>` CLI flag: bypass provisioning and read any index store directly (`main.swift`). → verify: reading Ice's and Food Truck's `Index.noindex/DataStore` produced correct symbols/edges, 0 unresolved.

## Phase 1 — Xcode discovery & mode selection

- [x] 1.1 `Provisioning.discoverProjects` finds `.xcodeproj`/`.xcworkspace` (bundle dirs; `.xcworkspace` wins over a co-located `.xcodeproj`) AND `Package.swift`, returning a `SwiftProject` enum; `main.swift` dispatches by kind; `--projects` entries are classified by extension. Discovery moved entirely into the sidecar (the Rust file-walk can't see bundle dirs); `KennSwift::resolve_projects` now just passes explicit projects through. → verify: Ice (`.xcodeproj`) → Xcode mode; fixture (`Package.swift`) → SwiftPM mode (EndToEndTests still green).
- [x] 1.2 Scheme via `xcodebuild -list -json`: pick the container-named scheme, else the first. → verify: Ice resolved scheme `Ice`; Food Truck resolved `Food Truck` (both built successfully).

## Phase 2 — Xcode build & store location

- [x] 2.1 `xcodebuild build` with the validated recipe (`-scheme`, `-derivedDataPath <container>/.kenn-xcode-dd`, auto destination from `SUPPORTED_PLATFORMS` or a `--platform` override, `CODE_SIGNING_ALLOWED=NO`, `COMPILER_INDEX_STORE_ENABLE=YES`); logs → stderr. → verify: Ice built `platform macosx` (auto), Food Truck built `iOS Simulator` (`--platform ios`), both `BUILD SUCCEEDED`.
- [x] 2.2 Read `<derivedDataPath>/Index.noindex/DataStore` via the shared `Indexer.collect`. → verify: Ice 2955 syms / 5129 edges, Food Truck 1510 / 2806, both 0 unresolved.
- [x] 2.3 Clear errors: no scheme, `xcodebuild` failure (with an iOS-runtime hint pointing at `xcodebuild -downloadPlatform iOS`), and no store produced → error frame, not a crash. → verify: the earlier no-runtime run emitted a clear error, not a crash.

## Phase 3 — verification

- [x] 3.1 macOS Xcode app (Ice), sidecar drove `xcodebuild` end-to-end: 154 files / 2955 syms / 5129 edges, 0 unresolved, real AppKit/SwiftUI conformances. → verify: done.
- [x] 3.2 iOS Xcode app (Food Truck, simulator) via the sidecar with `--platform ios`: 84 files / 1510 syms / 2806 edges, 0 unresolved, UIKit/SwiftUI conformances, actor→class, `@main` App. → verify: done.
- [x] 3.3 Quality gates: clippy clean, CRAP PASSED, `cargo fmt` no drift, `swift test` green, `just build-indexer-swift` builds. → verify: all green.

## Review fixes (post-review hardening)

- [x] R1 Derived data moved out of the user's repo tree into `<workspace>/.kenn/local/xcode-dd/<name>-<hash>` (kenn's local-artifacts convention); `discoverProjects` skips `.kenn`. → verify: Ice run wrote `.kenn/local/xcode-dd/Ice-85ca3e21/Index.noindex/DataStore`, read 2955 syms / 0 unresolved.
- [x] R2 `-skipPackageUpdates` on `-list`/`-showBuildSettings`/`build` to avoid re-resolving the SPM graph each call; `-showBuildSettings` is skipped entirely when `--platform` is explicit. → verify: flag accepted by `xcodebuild -list`.
- [x] R3 `classify`/`packageDir` tolerate a package *directory* path (not just `Package.swift`). → verify: unit test.
- [x] R4 Unit tests for the xcodebuild-free discovery logic (`ProvisioningTests`: `classify` by extension; `discoverProjects` finds both kinds, `.xcworkspace` wins, skips bundles/`.build`/`.kenn`). → verify: `swift test` green.

## Phase 4 — deferred polish (separate, optional)

- [x] 4.1 Filter macro/accessor noise via `isNoiseName` (`$s…` macro symbols incl. `#Preview` members by `$s`-rooted key; `getter:`/`setter:` accessors as defs AND as edge targets). → verify: Ice run has 0 `$s`/`getter:`/`setter:` keys, 0 unresolved.
- [x] 4.2 Wire `--platform` through `SwiftConfig` → `KennSwift` → both `build_driver` and `configure_runner` (sidecar `--platform` flag forwarded). → verify: config round-trips; clippy/CRAP/fmt green.
- [x] 4.3 (found via the imports probe) Workspace scoping excludes build-artifact trees (`/.build/`, `/.kenn/`, `/DerivedData/`) so dependency checkouts don't leak — fixes the regression from relocating derived data under `.kenn` AND a pre-existing SwiftPM `.build/checkouts` leak. → verify: Ice 154→116 files (0 dep), swift-nio 658→494 (0 dep), both 0 unresolved.
