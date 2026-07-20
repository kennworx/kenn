# swift-prepare-for-indexing

## Why

Swift is the only language where kenn's index quality depends on the target *compiling*: the sidecar runs `swift build`, and when a target fails, its dependent targets are skipped — their files end up absent from the store, or worse, served from stale units left by a previous build (wrong ranges, deleted symbols). Spikes on Swift 6.3.2 (`tmp/spike-swift/`, logs in `tmp/spike-*.log`) showed the fix is available today: `swift build --experimental-prepare-for-indexing` — the mechanism SourceKit-LSP's background indexing uses — exits 0 on non-compiling code, compiles *all* targets error-tolerantly (partial swiftmodules), writes the index store to the standard path, and the existing kenn-swift reader consumes it unchanged, symbols and edges complete for both the broken file and its dependents. It also skips codegen, so it's faster than the current full build.

## What Changes

- SwiftPM provisioning uses `swift build --experimental-prepare-for-indexing --build-tests` as the primary build; on non-zero exit (older toolchain without the flag, manifest failure) it falls back to the current plain `swift build --build-tests`, and only then to reading any existing store.
- The reader skips stale units — units whose main source file was modified after the unit was written (SourceKit-LSP's freshness rule) — and reports skipped files via a warning frame, instead of silently emitting symbols/ranges that describe old code. This protects the paths prepare-for-indexing can't fix: Xcode mode after a failed `xcodebuild`, `--skip-build`, and explicit `--store`.
- Not doing: `-continue-building-after-errors` (spike-proven not to rescue dependent targets — and per-file index emission is already error-tolerant on modern toolchains); an xcodebuild prepare equivalent (none exists; Xcode mode keeps the failure-reporting from `index-status-error-reporting` plus the staleness skip).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `swift-stream-indexer`: the provisioning requirement changes — SwiftPM mode MUST attempt the prepare-for-indexing build first with fallback to a plain build; and the reader MUST skip units older than their main source file, reporting them instead of emitting stale data.

## Impact

- `indexers/kenn-swift/Sources/kenn-swift/Provisioning.swift` — prepare-first build with fallback.
- `indexers/kenn-swift/Sources/kenn-swift/Indexer.swift` — mtime staleness check in the unit loop.
- No Rust-side changes; no reader format changes (spike-verified: prepare-mode units pass the existing `isSystem`/`isModule`/`mainFile` filters).
- Depends on `index-status-error-reporting` for the warning/error frames reaching `RunReport` — implement that first (frames are still emitted regardless; only their visibility depends on it).
- Toolchains without the flag (pre-Swift-6) transparently keep today's behavior via the fallback.
