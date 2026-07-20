## Why

`add-swift-index` indexes SwiftPM packages, but **most real iOS/macOS apps are
Xcode projects** (`.xcodeproj`/`.xcworkspace`), which `swift build` cannot build
(no `Package.swift`). So the apps that matter most are currently unreachable.

The good news, proven by a spike this session: the index store is the **same
libIndexStore format** whether produced by `swift build` or `xcodebuild` — both
just run `swiftc -index-store-path …`. So `Indexer.swift` (store → JSONL: symbols,
keys, edges, extension folding) needs **zero changes**. The entire gap is in
`Provisioning`: discovering the project, driving `xcodebuild`, and locating the
store.

The spike validated the full path end-to-end on real apps:

| App | Build | Store read |
|---|---|---|
| Ice (macOS, AppKit/SwiftUI) | `xcodebuild` ✓ | 116 files / 2238 syms / 4251 edges, 0 unresolved |
| Food Truck (iOS, UIKit/SwiftUI) | `xcodebuild -destination 'generic/platform=iOS Simulator'` ✓ | 80 files / 1334 syms / 2782 edges, 0 unresolved |

Both read correctly via a `--store <path>` primitive (already landed) that points
the existing reader at any store — real SwiftUI/AppKit/UIKit conformances
(`View`, `ObservableObject`, `Identifiable`, `PreviewProvider`), actors, and the
`@main` App struct all resolved.

## What Changes

Add an **Xcode mode** to the Swift sidecar's `Provisioning` — selected when the
discovered project is an `.xcodeproj`/`.xcworkspace` rather than a `Package.swift`.
The semantic core (`Indexer.swift`), the wire, keys, and edges are unchanged.

- **Discovery** — find `.xcodeproj`/`.xcworkspace` and enumerate schemes
  (`xcodebuild -list`). `Package.swift` discovery (SwiftPM mode) is unchanged;
  the two coexist.
- **Build** — drive `xcodebuild build` with the validated recipe: a chosen scheme,
  `-derivedDataPath <local>` (deterministic store location, sidesteps the hashed
  global DerivedData), `-destination 'generic/platform=iOS Simulator'` or
  `'generic/platform=macOS'`, `CODE_SIGNING_ALLOWED=NO`,
  `COMPILER_INDEX_STORE_ENABLE=YES`. Build logs go to stderr (stdout is the JSONL
  channel).
- **Store location** — read `<derivedDataPath>/Index.noindex/DataStore` (vs
  SwiftPM's `.build/<triple>/<config>/index/store`).
- **`--store <path>`** (already landed) — the read-primitive bypass that points
  the reader at any store; the foundation Xcode mode builds on.

## Capabilities

### Modified Capabilities

- `swift-stream-indexer`: add Xcode-project provisioning — `.xcodeproj`/
  `.xcworkspace` + scheme discovery, `xcodebuild` build with a local
  `derivedDataPath`, and reading `Index.noindex/DataStore`. The symbol/edge
  emission contract is unchanged (the reader is build-system agnostic).

## Impact

- **Reader unchanged** — `Indexer.swift`, wire, keys, edges all carry over; this
  is purely `Provisioning` + CLI flags.
- **New operational surface** (the reason it's a separate change): scheme
  selection, SDK/destination choice, the iOS **simulator runtime** must be
  installed (`xcodebuild -downloadPlatform iOS`, ~8.5 GB), and `xcodebuild`
  itself must be healthy (`-runFirstLaunch` after an Xcode update).
- **macOS-only** (xcodebuild requires Xcode; SwiftPM mode still runs on Linux).
- **Build cost/reliability** — apps take minutes and can fail without the right
  scheme/SDK; surface failures clearly and fall back to any existing store.
- **Out of scope:** code-signing/device builds (simulator + `CODE_SIGNING_ALLOWED=NO`
  only), CocoaPods/Carthage workspace nuances beyond what `xcodebuild` resolves,
  and `#Preview`-macro / accessor name-noise cleanup (tracked separately).
