# indexer-distribution Specification

## Purpose
TBD - created by archiving change indexer-formulas. Update Purpose after archive.
## Requirements
### Requirement: Each kenn-authored indexer is distributed as its own formula

kenn SHALL publish one Homebrew formula per indexer it authors — `kenn-ts`,
`kenn-dotnet`, `kenn-swift` — separate from the `kenn` formula.

The `kenn` formula SHALL NOT bundle them. The indexers total roughly 26 times
the size of the CLI, and bundling charges every user for languages they do not
index.

Each formula SHALL install exactly one executable, named as kenn probes for it
on `PATH`, so that installing the formula is sufficient for `kenn init` to
discover it with no further configuration.

#### Scenario: Installing the CLI alone

- **WHEN** a user runs `brew install kennworx/tap/kenn`
- **THEN** the `kenn` executable is installed
- **AND** no indexer binary is installed

#### Scenario: Installing one indexer

- **WHEN** a user runs `brew install kennworx/tap/kenn-dotnet`
- **THEN** the `kenn-dotnet` executable is on `PATH`
- **AND** a subsequent `kenn init` in a C# workspace enables `[language.csharp]`
  rather than degrading it

### Requirement: A failing indexer build never blocks the release

Each indexer SHALL be built and published independently. A failure building or
publishing one indexer SHALL NOT prevent the `kenn` release, nor the release of
any other indexer.

A release in which some indexer failed SHALL still publish everything that
succeeded, and the failure SHALL be visible rather than silent.

#### Scenario: One indexer fails to build

- **WHEN** the `kenn-swift` build fails during a tagged release
- **THEN** the `kenn` formula is still published
- **AND** the `kenn-ts` and `kenn-dotnet` formulas are still published
- **AND** the release reports the `kenn-swift` failure

### Requirement: Formula checksums come from the published artifacts

A generated formula SHALL take each checksum from the `.sha256` file uploaded
alongside the artifact it describes, and SHALL NOT recompute it from a local
build.

Generation SHALL fail, with a non-zero exit status, when any required checksum
is missing or empty. A formula MUST NOT be published with an absent or empty
`sha256` field.

#### Scenario: A missing checksum aborts generation

- **WHEN** formula generation runs and one platform's `.sha256` file is absent
- **THEN** generation exits non-zero
- **AND** no formula is written or pushed

#### Scenario: Published formula matches published artifact

- **WHEN** a formula is published for a release
- **THEN** each `sha256` in it equals the checksum of the archive at the
  corresponding URL

### Requirement: An installed indexer runs without further setup

An indexer installed from a formula SHALL execute on a clean machine of that
platform, resolving every library it links, with no toolchain the formula does
not declare or provide.

A formula SHALL NOT be published for a platform where its binary cannot resolve
its libraries. `kenn-swift` links `libIndexStore`, which is part of the Swift
toolchain rather than the OS on both macOS and Linux, so it either vendors that
library with a corrected load path or declares the toolchain as a dependency.

An unresolved library is the failure this requirement exists to prevent: the
binary installs successfully and dies at index time naming neither the missing
library nor the reason.

#### Scenario: Swift indexer on a machine without Xcode

- **WHEN** `kenn-swift` is installed from the tap on macOS with only the
  Command Line Tools present
- **THEN** the binary executes and reports its version
- **AND** it does not fail resolving `libIndexStore`

#### Scenario: Swift indexer on Linux

- **WHEN** a Linux user attempts to install `kenn-swift` from the tap
- **THEN** Homebrew reports the formula is unavailable for that platform
- **AND** the message names the docker runtime and building from source as the
  supported routes

#### Scenario: A self-contained indexer needs no runtime

- **WHEN** `kenn-dotnet` is installed on a machine with no .NET SDK
- **THEN** it executes and reports its version

### Requirement: Indexers are versioned with the CLI

Every indexer SHALL be released at the same version as the `kenn` CLI it ships
alongside, from the same tag.

This is what makes the wire protocol between CLI and indexer an internal
detail: a user cannot end up pairing versions that were never built together,
so no compatibility matrix is required.

#### Scenario: A tagged release publishes matching versions

- **WHEN** `v1.2.3` is released
- **THEN** each published indexer formula declares version `1.2.3`

