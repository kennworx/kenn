## ADDED Requirements

### Requirement: A Swift toolchain that satisfies the declared minimum is reused

kenn SHALL treat a Swift `swift-tools-version` declaration as a **minimum**, not an exact
version: if a Swift toolchain whose version is `>=` the declared version is already
provisioned in the shared cache, kenn SHALL reuse it rather than provisioning the exact
declared version. Preference is the
exact declared version when present, otherwise the highest provisioned version that
satisfies the minimum (cross-major permitted, since a newer toolchain builds an older
tools-version in the older language mode). Only when no provisioned toolchain satisfies
the minimum SHALL the declared version be provisioned. Every other language continues to
require its exact declared version.

The host preflight and the in-container entrypoint SHALL apply the identical selection
rule over the same cache, so both agree on which toolchain runs; the version actually
used SHALL be reported (wire frame and run metadata), which may be higher than the
declared minimum.

#### Scenario: A higher provisioned Swift toolchain satisfies a lower pin

- **WHEN** a workspace declares `swift-tools-version:6.0`
- **AND** the shared cache already holds a provisioned swift `6.3` but no swift `6.0`
- **THEN** kenn reuses `6.3` without pulling or provisioning `6.0`
- **AND** indexing produces the workspace's symbols, attributed to swift `6.3`

#### Scenario: The exact declared Swift version is preferred when present

- **WHEN** a workspace declares `swift-tools-version:6.0`
- **AND** the cache holds both swift `6.0` and swift `6.3`
- **THEN** kenn uses `6.0`

#### Scenario: No satisfying toolchain means the declared version is provisioned

- **WHEN** a workspace declares `swift-tools-version:6.5`
- **AND** the cache holds only swift `6.3`
- **THEN** kenn provisions swift `6.5` from the official image and uses it
