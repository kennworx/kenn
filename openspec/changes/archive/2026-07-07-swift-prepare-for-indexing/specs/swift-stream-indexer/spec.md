## MODIFIED Requirements

### Requirement: The Swift sidecar discovers SwiftPM packages and provisions an index store

The Swift indexer SHALL provision an index store from either build system and
select the mode by discovered project type: a `Package.swift` selects SwiftPM
mode, while an `.xcodeproj`/`.xcworkspace` selects Xcode mode. In SwiftPM mode
the indexer SHALL first attempt `swift build
--experimental-prepare-for-indexing --build-tests` (error-tolerant, compiles
all targets to partial swiftmodules even when the code does not compile, and
populates `.build/<triple>/<config>/index/store`); when that exits non-zero
(toolchain without the flag, manifest or resolution failure) it SHALL fall
back to a plain `swift build --build-tests`, and only after both fail read any
existing store. In Xcode mode the indexer SHALL
run `xcodebuild build` for a chosen scheme with `-derivedDataPath <local>`,
`COMPILER_INDEX_STORE_ENABLE=YES`, and `CODE_SIGNING_ALLOWED=NO`, targeting
`generic/platform=macOS` for a Mac app or `generic/platform=iOS Simulator` for
iOS, then read `<derivedDataPath>/Index.noindex/DataStore`. A `--skip-build`
equivalent and an explicit `--store <path>` SHALL allow reading an existing store
without building. Build output SHALL go to stderr (stdout is the JSONL channel),
and a missing iOS simulator runtime or an absent store SHALL produce a clear
error rather than a crash. The symbol/edge emission contract is unchanged across
modes — the reader is build-system and platform agnostic.

#### Scenario: a non-compiling package is still fully indexed

- **WHEN** a SwiftPM package has a target with a type error and a second
  target that depends on it
- **THEN** the prepare-for-indexing build exits 0 and the sidecar emits
  symbols and edges for the files of **both** targets, including the file
  containing the error

#### Scenario: an old toolchain falls back to a plain build

- **WHEN** the installed `swift` does not support
  `--experimental-prepare-for-indexing` and the package compiles
- **THEN** the sidecar falls back to `swift build --build-tests` and indexes
  the resulting store exactly as before this change

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

## ADDED Requirements

### Requirement: Stale units are handled per provisioning outcome

The reader SHALL classify a unit as mtime-stale when its main source file
is STRICTLY newer than the unit file (equal mtimes are fresh — a source
written and compiled within one clock tick on a coarse-mtime filesystem
must not be dropped), and as deleted-source when the main file no longer
exists. Deleted-source units SHALL be skipped in EVERY checking mode —
deletion is unambiguous, and emitting such a unit would pair an
empty-bytes `content_hash` with ranges into a gone file. Mtime-stale
handling SHALL depend on how the store was provisioned:

- **After a FAILED in-process build** (the store is a fallback read):
  mtime-stale units SHALL be skipped — their ranges may describe edited
  code — and reported. EXCEPT when the staleness is systematic: when
  strictly more than half of the units whose source still exists are
  mtime-stale, they SHALL be kept and the skew reported instead — mass
  staleness signals checkout/cache mtime noise (fresh clone over a CI
  cache, archive/rsync/Docker restore), not real edits, and dropping
  them all would empty the index.
- **On trusted-store reads** (`--skip-build`, `--store`): mtime-stale
  units SHALL be kept and reported. Source mtimes routinely postdate an
  externally built store (fresh checkout of a CI artifact); skipping
  would empty the index.
- **After a successful in-process build**: no check — every unit is
  fresh by construction.

The report SHALL name deleted-source and mtime-stale counts separately —
they mean different things to someone triaging `kenn status`.

Stale units SHALL be surfaced per project via an `ErrorFrame{severity:
"warning", source: "store"}` carrying the count and example paths; they
SHALL NOT fail the run. When the store's units directory is not at the
expected layout (`<store>/v5/units`), the reader SHALL say so via a
warning frame and disable the check rather than silently treating
everything as fresh.

#### Scenario: a stale unit after a failed Xcode build is skipped and reported

- **WHEN** an `xcodebuild` provisioning build fails and the derived-data store
  holds a unit written before the unit's main source file was last modified
- **THEN** the reader emits no symbols from that unit
- **AND** emits a warning frame naming the project and the stale count

#### Scenario: systematic mtime skew does not empty the index in skip mode

- **WHEN** an in-process build fails, the fallback store predates a fresh
  checkout, and (strictly) more than half of the checked units are
  mtime-stale
- **THEN** the mtime-stale units are kept and a warning frame reports the
  skew, while units with deleted sources are still skipped

#### Scenario: a trusted store older than the checkout still indexes fully

- **WHEN** a prebuilt store is read via `--store` (or `--skip-build`) in a
  checkout whose source mtimes all postdate the store's unit files
- **THEN** every unit whose source still exists is emitted (no blackout)
- **AND** a warning frame reports how many units are older than their
  sources

#### Scenario: a deleted source is dropped even on a trusted read

- **WHEN** a `--skip-build` read encounters a unit whose main source file
  was deleted after the store was built
- **THEN** the unit is skipped and the warning frame names the
  deleted-source count distinctly

#### Scenario: fresh units are unaffected

- **WHEN** every unit in the store is newer than its main source file
- **THEN** the emitted wire is identical to today's output (no warning frame)
