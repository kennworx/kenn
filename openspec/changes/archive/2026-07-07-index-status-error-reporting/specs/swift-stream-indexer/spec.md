## ADDED Requirements

### Requirement: A failed provisioning build is reported on the wire

The sidecar SHALL emit an `ErrorFrame{severity: "error", source: "build"}`
whose `path` names the package or project directory whenever the
provisioning build it runs itself (`swift build` in SwiftPM mode,
`xcodebuild` in Xcode mode) exits non-zero, before falling back to reading
any existing index store. When no index store exists at all for a discovered
project, the sidecar SHALL emit an `ErrorFrame{severity: "error"}` for that
project (today this is a stderr log only). Build-failure diagnostics on
stderr are unchanged; the error frame is additive. The sidecar SHALL still
exit 0 when it produced any frames, preserving partial output — status
degradation is the consumer's job (an error frame degrades the unit report
to `Partial` per the jsonl-indexer-driver spec).

#### Scenario: a failed swift build becomes a Partial unit, not a silent Success

- **WHEN** `swift build` exits non-zero for a package and a previous index
  store exists on disk
- **THEN** the sidecar emits `ErrorFrame{severity:"error", source:"build",
  path:<package dir>}` and continues reading the existing store
- **AND** the resulting `RunReport` status is `Partial` with the package
  listed in `failed_projects`

#### Scenario: a missing store is an error frame, not only a log line

- **WHEN** the build fails and no index store exists under the package's
  build directory
- **THEN** the sidecar emits an `ErrorFrame{severity:"error"}` naming the
  package and emits zero symbols for it
