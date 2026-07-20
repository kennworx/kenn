---
id: fnd_527a41b7-a9be-40ae-ac80-cdd61edb4077
tags:
- guide
- swift
parent_ids: []
created_at: 2026-06-20T12:14:25.725401Z
---
kenn-swift is SwiftPM-only today, but the SwiftPM/Xcode split is ENTIRELY in `Provisioning.swift` — the index store is the same libIndexStore unit/record format whichever build system produced it, so `Indexer.swift` (store → JSONL: symbols, keys, edges, extension folding) works unchanged on an Xcode-produced store.

The provisioning differences for a future Xcode mode:
- Discovery: `.xcodeproj`/`.xcworkspace` (+ `xcodebuild -list` for schemes) instead of `Package.swift`.
- Build: `xcodebuild` (needs a scheme + SDK/destination — target the SIMULATOR for iOS to avoid code signing) instead of `swift build`.
- Store location: Xcode writes to `~/Library/Developer/Xcode/DerivedData/<Proj>-<hash>/Index.noindex/DataStore` (hashed, global — find it via `xcodebuild -showBuildSettings`'s BUILD_ROOT/OBJROOT), whereas SwiftPM writes `<pkg>/.build/<triple>/<config>/index/store`.

So "support all iOS/macOS apps" is a Provisioning extension (discovery + build driver + store location), not a rewrite. Note `swift build` literally cannot build an `.xcodeproj` (no Package.swift) — they are separate build systems; this is not a kenn limitation. `--build-tests` (SwiftPM) is what pulls test-target symbols into the store; the Xcode analog is building the test scheme.