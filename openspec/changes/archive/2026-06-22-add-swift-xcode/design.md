## Context

Xcode and SwiftPM are separate build systems, but they feed the *same* index
store (libIndexStore unit/record format) because both ultimately run
`swiftc -index-store-path …`. The spike confirmed `Indexer.swift` reads an
Xcode-produced store with no changes — so Xcode support is a `Provisioning`
front-end, not a reader change.

```
   SwiftPM mode ─ swift build ──────────────────▶ .build/<triple>/<cfg>/index/store ┐
                                                                                     ├─▶ Indexer.swift (UNCHANGED)
   Xcode mode  ─ xcodebuild -derivedDataPath D ─▶ D/Index.noindex/DataStore ────────┘
```

Validated recipe (Ice/macOS, Food Truck/iOS), deterministic + signing-free:

```
xcodebuild build -project X.xcodeproj -scheme S \
  -destination 'generic/platform=iOS Simulator'   # or 'generic/platform=macOS'
  -derivedDataPath <local> CODE_SIGNING_ALLOWED=NO COMPILER_INDEX_STORE_ENABLE=YES
→ read <local>/Index.noindex/DataStore
```

## Goals / Non-Goals

**Goals:**

- Index Xcode `.xcodeproj`/`.xcworkspace` apps (iOS + macOS) reusing the existing
  reader verbatim.
- Deterministic store location via a local `-derivedDataPath` (no hashed-global
  DerivedData hunt).
- Coexist with SwiftPM mode; pick per discovered project type.

**Non-Goals:**

- Code-signing / on-device builds (simulator + `CODE_SIGNING_ALLOWED=NO` only).
- Reader/wire/key/edge changes (none needed).
- `#Preview`-macro `$s…` name noise and stray `getter:` accessor edges — real but
  cosmetic, tracked as separate polish.
- Auto-installing the iOS runtime or repairing `xcodebuild` — environment
  prerequisites, surfaced as clear errors, not done by the sidecar.

## Decisions

### D1 — Mode selection by discovered project type

`Provisioning` discovers `Package.swift` (SwiftPM mode) OR `.xcodeproj`/
`.xcworkspace` (Xcode mode). Explicit `--projects` may name either. A workspace
takes precedence over a bare project in the same dir (it aggregates them).

### D2 — `xcodebuild` with a local `-derivedDataPath`

Always pass `-derivedDataPath <run-local dir>`: it makes the store path
deterministic (`<dir>/Index.noindex/DataStore`) and avoids parsing the hashed
`~/Library/Developer/Xcode/DerivedData/<Proj>-<hash>` path. Pass
`COMPILER_INDEX_STORE_ENABLE=YES` (store on) and `CODE_SIGNING_ALLOWED=NO`
(simulator/local builds need no signing). Build output → stderr.

### D3 — Destination: simulator for iOS, generic for macOS

`-destination 'generic/platform=iOS Simulator'` for iOS (no booted device, no
signing; requires the iOS **runtime** installed — surface a clear error pointing
at `xcodebuild -downloadPlatform iOS` if absent). `'generic/platform=macOS'` for
Mac apps. Other platforms (tvOS/watchOS/visionOS) are the same pattern, deferred.

### D4 — Scheme selection

Default to the scheme matching the project name, else the first shared scheme
from `xcodebuild -list`; allow an explicit `--scheme`. A `.xcworkspace` is built
with `-workspace` + scheme. Skip schemes that build nothing indexable.

### D5 — Store location + the `--store` primitive (landed)

The reader already accepts `--store <path>` (bypasses provisioning, reads any
store). Xcode mode computes `<derivedDataPath>/Index.noindex/DataStore` and feeds
it to the same `Indexer.collect`. `--store` stays as the manual escape hatch and
the seam these tests used.

### D6 — Workspace scoping unchanged

`--workspace` still scopes which files count (prefix match on resolved paths), so
SPM-dependency sources checked out under `<derivedDataPath>/SourcePackages` are
naturally excluded — verified on Food Truck (0 dependency files leaked).

## Open Questions

- **Freshness**: when to rebuild vs. read an existing DerivedData store (same
  question as SwiftPM mode; `xcodebuild` is incremental so always-build is
  cheap-when-fresh, but slower to spawn than `swift build`).
- **Multi-scheme apps**: index one scheme or union several? Start with one.
- **`xcodebuild` health**: a stale post-update Xcode fails to load plugins
  (`-runFirstLaunch` fixes it). Detect and surface, don't auto-repair.
- **`.xcworkspace` + Pods**: RESOLVED — validated on a CocoaPods-native app
  (Toast-Swift demo + SwiftyJSON): the sidecar discovers the `.xcworkspace`,
  builds with `-workspace`, and indexes the app. `Pods/` (vendored dependency
  source, in-tree) is EXCLUDED by the reader's scope filter — parity with how
  SPM `.build/checkouts` deps are skipped (kenn indexes the project, not its
  deps). Required adding `ENABLE_USER_SCRIPT_SANDBOXING=NO` to the build — Xcode
  15+ sandboxes run-script phases, which otherwise denies CocoaPods'
  embed-frameworks `rsync` (the "Sandbox: rsync deny" failure). `pod install`
  must have run first (out of scope for the sidecar). A project with a
  pre-existing per-config base xcconfig (e.g. Apple's Food Truck sample) can
  leave `${PODS_ROOT}` undefined — that's a CocoaPods/project integration quirk
  (plain `xcodebuild` fails identically), not a kenn issue.
