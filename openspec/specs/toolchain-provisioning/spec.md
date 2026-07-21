# toolchain-provisioning Specification

## Purpose

kenn provisions each language's workspace-pinned toolchain on demand at index
time, rather than baking a toolchain into the indexer image. The version a
workspace declares for itself (via `global.json`, `rust-toolchain.toml`,
`go.mod`, `Package.swift`, `.python-version`) is resolved and installed into a
shared, content-addressed cache volume keyed by resolved version and
architecture, so a given toolchain is downloaded once per machine and reused
across every workspace. Each downloaded artifact is verified against the
integrity value its vendor publishes (carrying its own algorithm — SHA-256 for
Rust, Go, Node, and python-build-standalone; SHA-512 for .NET) before it is
unpacked, and installation is atomic so a partial install is never treated as
usable. An unresolvable or uninstallable pin is a fatal, named error: kenn fails
the run non-zero quoting the declared version and its source file, and never
falls back to a different toolchain or reports a successful, empty index.

## Requirements

### Requirement: The workspace's declared toolchain version is what runs

kenn SHALL read the toolchain version a workspace declares for itself and use
that version to index it. Where a language declares a version in a pin file
(`global.json`, `rust-toolchain.toml`, `go.mod`, `Package.swift`,
`.python-version`), that declaration SHALL determine the toolchain, not the
version present in an image or on the host.

#### Scenario: A pinned major that is not the newest is honored

- **WHEN** a workspace pins an SDK major older than the newest kenn knows about
- **THEN** the pinned major is provisioned and used
- **AND** indexing produces the workspace's symbols rather than zero files

#### Scenario: The nearest pin file wins

- **WHEN** a pin file exists both at the workspace root and in a nested directory
  being indexed
- **THEN** the nearest pin file determines the toolchain version

#### Scenario: No pin means the default toolchain

- **WHEN** a workspace declares no toolchain version
- **THEN** kenn provisions its default version for that language and indexes
  successfully

### Requirement: An unusable toolchain fails loudly and names the pin

When the declared toolchain cannot be resolved or provisioned, kenn SHALL fail
the run with a non-zero exit, quoting the declared version and naming the file it
was read from. kenn SHALL NOT fall back to a different toolchain version, and
SHALL NOT report success with an empty or partial index.

#### Scenario: An unresolvable pin aborts the run

- **WHEN** a workspace pins a toolchain version that cannot be resolved
- **THEN** the run exits non-zero
- **AND** the diagnostic contains the pinned version and its source file path
- **AND** no index is written that would report the workspace as successfully
  indexed

#### Scenario: A provisioning failure is not silently absorbed

- **WHEN** provisioning fails partway through, for example the download is
  interrupted
- **THEN** the run exits non-zero with the cause
- **AND** a subsequent run does not treat the partial installation as usable

### Requirement: Toolchains are cached once per machine and shared

Provisioned toolchains SHALL be stored in a cache keyed by language and resolved
version, shared across every workspace on the machine. A toolchain already
present SHALL NOT be downloaded again.

#### Scenario: A second workspace reuses a provisioned toolchain

- **WHEN** two workspaces resolving to the same toolchain version are indexed in
  sequence
- **THEN** the second indexes without downloading the toolchain again

#### Scenario: Distinct pins resolving to one version share an install

- **WHEN** two workspaces declare the toolchain differently but resolve to the
  same concrete version
- **THEN** both use a single cached installation

#### Scenario: A warm cache indexes without network access

- **WHEN** a workspace whose toolchain is already cached is indexed with network
  access disabled
- **THEN** indexing completes successfully

### Requirement: Concurrent provisioning is safe

Provisioning SHALL be safe against concurrent kenn runs targeting the same
toolchain. Installation SHALL be atomic: the cache SHALL never expose a
partially-populated toolchain to a reader.

#### Scenario: Two runs provision the same toolchain at once

- **WHEN** two kenn runs requiring the same uncached toolchain start concurrently
- **THEN** both complete successfully
- **AND** the cache holds one complete installation

#### Scenario: An interrupted install leaves no usable remains

- **WHEN** a provisioning run is interrupted mid-installation
- **THEN** the next run does not treat the interrupted installation as complete
- **AND** it reprovisions and succeeds

### Requirement: Provisioning reports progress before it begins

kenn SHALL signal that provisioning has started before the download begins, and
SHALL signal its completion. Provisioning downloads hundreds of megabytes and can
take minutes; a silent producer during this phase is indistinguishable from a
hung one.

#### Scenario: The start signal precedes the download

- **WHEN** a run provisions an uncached toolchain
- **THEN** the consumer observes the provisioning-started signal before any
  download progresses
- **AND** observes a completion signal before indexing begins

### Requirement: A downloaded artifact is verified before it is unpacked

The provisioner SHALL verify every downloaded artifact against the integrity
value the vendor publishes for it, and SHALL refuse to unpack an artifact that
cannot be verified. Transport security authenticates the server, not the bytes;
an unverified artifact is never treated as good enough.

Vendors do not agree on an algorithm, so the expected value SHALL carry its
algorithm rather than being assumed. Measured across the six toolchains: Rust,
Go, Node and python-build-standalone publish SHA-256; **.NET publishes SHA-512**
and serves no SHA-256 sidecar. Assuming one algorithm would either drop
verification for .NET or compare across hash families and always fail.

A language whose vendor publishes no machine-readable integrity value at all
SHALL NOT be provisioned until an explicit verification decision is recorded for
it. Silently downgrading such a language to "TLS was fine" is prohibited.

#### Scenario: A mismatched artifact is refused

- **WHEN** a downloaded artifact's hash does not match the published value
- **THEN** the install fails with a non-zero exit naming the algorithm and the
  mismatch
- **AND** nothing is unpacked into the cache

#### Scenario: A missing published value is a failure, not a warning

- **WHEN** no published integrity value can be obtained for an artifact
- **THEN** the install fails rather than proceeding unverified

#### Scenario: A vendor's own algorithm is used, not an assumed one

- **WHEN** a vendor publishes its artifact hash as SHA-512
- **THEN** verification computes SHA-512 and compares against that value
- **AND** a value from a different hash family does not verify

### Requirement: A local runtime provisions no toolchain

Under `runtime = "local"` kenn SHALL NOT install a toolchain. Provisioning
happens inside the indexer container, so a local run has none: it uses the
toolchain already present on the machine, and reports a missing or mismatched one
rather than installing over it.

#### Scenario: A local run reports rather than installs

- **WHEN** a local run needs a toolchain version that is not installed on the
  machine
- **THEN** kenn installs nothing
- **AND** it reports the missing toolchain with actionable guidance
