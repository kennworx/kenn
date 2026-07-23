# windows-platform-support Specification

## Purpose
TBD - created by archiving change windows-support. Update Purpose after archive.
## Requirements
### Requirement: kenn runs on Windows without elevation

kenn SHALL index, query, and roll back a workspace on Windows without
Administrator rights and without Developer Mode.

No kenn code path SHALL depend on creating a filesystem link. The store
uses a pointer file for `live` precisely because `symlink_dir` requires
elevation on Windows (see the `store-layout` capability).

#### Scenario: indexing succeeds as an unprivileged user

- **WHEN** an unelevated user runs `kenn index` on Windows with
  Developer Mode disabled
- **THEN** the run completes and `live` resolves to the new run
- **AND** `kenn status` reports that run

#### Scenario: rollback succeeds as an unprivileged user

- **WHEN** an unelevated user runs `kenn rollback` on Windows
- **THEN** `live` resolves to the prior run

### Requirement: The workspace builds and is checked on Windows in CI

The workspace SHALL compile for `x86_64-pc-windows-msvc`, and CI SHALL
verify this on every pull request.

A Windows build failure MUST surface as a pull-request failure, never
first as a failed release: the release matrix runs only on a tag, and a
target that fails there blocks the entire release including publication
of artifacts for platforms that built successfully.

#### Scenario: a unix-only API reaching the workspace fails CI

- **WHEN** a change introduces a `std::os::unix` call not behind a
  `cfg` gate with a working non-unix counterpart
- **THEN** the Windows CI job fails on that pull request

#### Scenario: Windows re-enters the release matrix only once proven

- **WHEN** `x86_64-pc-windows-msvc` is listed in the release targets
- **THEN** the Windows CI check is green
- **AND** a tagged release has produced a Windows artifact

### Requirement: Platform-specific filesystem facts are cfg-gated with real counterparts

Every `cfg` branch SHALL provide a working implementation of the
platform-specific filesystem fact it guards. A branch that
unconditionally returns an error SHALL NOT be used to stand in for an
unimplemented platform.

Such a branch compiles, passes review, and fails only at runtime on the
platform nobody tests — which is how the POSIX-only flip reached a
release matrix. Where a platform genuinely cannot support a capability,
that limitation belongs in a specification, not in an error string.

#### Scenario: same-filesystem detection works on Windows

- **WHEN** kenn decides whether a temporary directory and its
  destination are on the same filesystem on Windows
- **THEN** the decision is made from the canonicalised volume prefix
- **AND** an indeterminate result is treated as "same filesystem", so
  a misclassification surfaces as a loud rename failure rather than a
  silent fallback

### Requirement: The Docker indexer runtime is unsupported on Windows

kenn SHALL NOT select `runtime = "docker"` on Windows: the published
indexer images are Linux-only.

Windows users SHALL use local toolchains, or Docker Desktop with a WSL2
backend where the Linux images run unchanged.

#### Scenario: init does not offer docker on Windows

- **WHEN** `kenn init --docker` runs on Windows
- **THEN** the docker runtime is not selected for any language
- **AND** the output states that the indexer images are Linux-only and
  names the alternatives

