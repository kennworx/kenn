## 1. Prepare-first provisioning (SwiftPM mode)

- [x] 1.1 `runSwiftBuild` attempts `swift build --experimental-prepare-for-indexing --build-tests` first; on non-zero exit, retry with the current plain `swift build --build-tests`; keep existing failed-build handling (error frame + existing-store read) as the final fallback — `indexers/kenn-swift/Sources/kenn-swift/Provisioning.swift`
- [x] 1.2 Integration test: fixture package with a type error in one target and a dependent second target — assert exit 0 wire contains symbols and edges for both targets' files (mirror the spike: `tmp/spike-swift/`)
- [x] 1.3 Fallback test: simulate flag-unsupported (e.g. inject a wrapper that rejects the flag, or gate on toolchain) — assert plain build runs and output matches pre-change behavior on a compiling fixture

## 2. Stale-unit skip in the reader

- [x] 2.1 In the unit loop, skip units whose `mainFile` mtime is newer than the unit file's mtime (or whose `mainFile` no longer exists); count per project — `indexers/kenn-swift/Sources/kenn-swift/Indexer.swift`
- [x] 2.2 Emit one `ErrorFrame{severity:"warning", source:"store"}` per project with the stale count and a few example paths
- [x] 2.3 Test: build fixture, then modify a source file without rebuilding, read with `--skip-build` — assert the modified file's symbols are absent and the warning frame is present; unmodified files unaffected

## 3. Verify quality parity

- [x] 3.1 Compare symbol/edge counts on the kenn-swift test fixture between a plain-build store and a prepare-for-indexing store — investigate any drop (especially `calls` edges, since prepare mode may skip function bodies in some configurations)
- [x] 3.2 Run `just test-indexer-swift` green; end-to-end `kenn index` on a Swift workspace with a deliberately broken target publishes a snapshot whose Swift coverage includes the broken target's files

## 4. Housekeeping

- [x] 4.1 Update `swift-stream-indexer` spec Purpose (currently "TBD") while touching the spec at archive time
- [x] 4.2 Remove the spike directory `tmp/spike-swift/` and `tmp/spike-*.log` once 1.2's fixture reproduces its findings
- [x] 4.3 Fix workspace-root normalization surfaced by 1.2's tests: `resolvingSymlinksInPath()` strips `/private` while the compiler records realpaths, so a workspace under `/tmp`/`/var` indexed zero files — `main.swift` now uses POSIX `realpath` (`canonicalWorkspaceRoot` in `Sink.swift`), with a regression test

## 5. Review fixes (post-implementation code review)

- [x] 5.1 Staleness modes: skip only after a FAILED in-process build; `--skip-build`/`--store` keep stale units and warn (fixes the empty-index blackout on stores older than a fresh checkout); no check after a successful build
- [x] 5.2 Bare `stat(2)` mtimes with per-file memoization; warning frame when the store layout lacks `v5/units` (check disabled loudly, not silently)
- [x] 5.3 Equal-mtime doc comment corrected (equal ⇒ fresh, deliberately); EndToEndTests moved to `canonicalWorkspaceRoot`; shared `runReaderObjects` test harness extracted (TestSupport.swift)
- [x] 5.4 Portability (mac/linux/windows now required): mtime via `st_mtimespec`/`st_mtim`/Foundation per platform; `canonicalWorkspaceRoot` realpath on POSIX, Foundation on Windows; `swift` resolved from PATH portably (no `/usr/bin/env`); POSIX-only test shim gated `#if !os(Windows)`; Linux build verified in Docker

## 6. Third-review fixes

- [x] 6.1 Deleted-source units skipped in EVERY checking mode (warnOnly emitted them with empty-bytes content_hash); reported distinctly from mtime staleness in all message branches
- [x] 6.2 Ratio-guard denominator excludes deleted-source units (mass deletions no longer dilute skew detection)
- [x] 6.3 Skip-mode staleness is a single-parse pre-pass buffering (mainFile, module, recordName, freshness) — no double UnitReader construction or doubled unreadable-unit warnings; unit filter shared via `workspaceMainFile`; ingest + warning builder extracted from `collect`
- [x] 6.4 `findOnPath` mirrors execvp: skips directories (isExecutableFile is true for dirs via the search bit) and non-files, keeps scanning; Windows accepts only `.exe` (CreateProcessW can't spawn `.cmd`/`.bat`) via existence check
- [x] 6.5 `.build-*/` gitignored and the Docker scratch dir removed; justfile Windows claim hedged; `makeSwiftPackage`/`realPath` hoisted to TestSupport; deleted-source warnOnly test scenario added
