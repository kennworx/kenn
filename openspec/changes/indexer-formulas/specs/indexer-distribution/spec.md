## ADDED Requirements

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

### Requirement: Indexer platform support is declared, not implied

Each indexer formula SHALL declare the platforms it supports, and SHALL be
absent for platforms where its binary cannot run.

`kenn-swift` SHALL be macOS-only: it links the Swift runtime, which macOS
provides and Linux does not. Publishing a Linux `kenn-swift` that cannot exec
is worse than publishing none, because the failure appears at index time rather
than install time.

#### Scenario: Swift indexer on Linux

- **WHEN** a Linux user attempts to install `kenn-swift` from the tap
- **THEN** Homebrew reports the formula is unavailable for that platform
- **AND** the message names the docker runtime and building from source as the
  supported routes

### Requirement: Indexers are versioned with the CLI

Every indexer SHALL be released at the same version as the `kenn` CLI it ships
alongside, from the same tag.

This is what makes the wire protocol between CLI and indexer an internal
detail: a user cannot end up pairing versions that were never built together,
so no compatibility matrix is required.

#### Scenario: A tagged release publishes matching versions

- **WHEN** `v1.2.3` is released
- **THEN** each published indexer formula declares version `1.2.3`
