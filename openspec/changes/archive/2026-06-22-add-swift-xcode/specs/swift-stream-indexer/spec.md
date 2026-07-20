## MODIFIED Requirements

### Requirement: The Swift sidecar discovers SwiftPM packages and provisions an index store

The Swift indexer SHALL provision an index store from either build system and
select the mode by discovered project type: a `Package.swift` selects SwiftPM
mode (`swift build` → `.build/<triple>/<config>/index/store`), while an
`.xcodeproj`/`.xcworkspace` selects Xcode mode. In Xcode mode the indexer SHALL
run `xcodebuild build` for a chosen scheme with `-derivedDataPath <local>`,
`COMPILER_INDEX_STORE_ENABLE=YES`, and `CODE_SIGNING_ALLOWED=NO`, targeting
`generic/platform=macOS` for a Mac app or `generic/platform=iOS Simulator` for
iOS, then read `<derivedDataPath>/Index.noindex/DataStore`. A `--skip-build`
equivalent and an explicit `--store <path>` SHALL allow reading an existing store
without building. Build output SHALL go to stderr (stdout is the JSONL channel),
and a missing iOS simulator runtime or an absent store SHALL produce a clear
error rather than a crash. The symbol/edge emission contract is unchanged across
modes — the reader is build-system and platform agnostic.

#### Scenario: an Xcode macOS project is built and indexed

- **WHEN** the workspace contains a `.xcodeproj` with a buildable macOS scheme
- **THEN** the indexer builds it with `xcodebuild` to a local derived-data dir and
  reads `Index.noindex/DataStore`, emitting symbols and edges

#### Scenario: an Xcode iOS project builds against the simulator without signing

- **WHEN** an iOS `.xcodeproj` is indexed and the iOS simulator runtime is present
- **THEN** the indexer builds with `-destination 'generic/platform=iOS Simulator'`
  and `CODE_SIGNING_ALLOWED=NO` and indexes the resulting store

#### Scenario: a store can be read directly, bypassing the build

- **WHEN** `--store <path>` points at an existing `Index.noindex/DataStore`
- **THEN** the indexer reads it directly and emits the same wire as a built store

#### Scenario: a missing iOS runtime is reported, not crashed

- **WHEN** an iOS build finds no eligible destination because the simulator
  runtime is not installed
- **THEN** the indexer emits a clear error (pointing at `xcodebuild
  -downloadPlatform iOS`) and does not crash

#### Scenario: dependency sources are scoped out

- **WHEN** an Xcode project resolves SwiftPM dependencies under the derived-data
  `SourcePackages` directory
- **THEN** those dependency files are excluded from the index by the workspace
  scoping (only the app's own sources are emitted)
