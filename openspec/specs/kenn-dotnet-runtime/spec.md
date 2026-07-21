# kenn-dotnet-runtime Specification

## Purpose
TBD - created by archiving change kenn-dotnet-net10. Update Purpose after archive.
## Requirements
### Requirement: net10-aligned toolchain

The kenn-dotnet indexer SHALL target the .NET 10 generation across every layer
that participates in MSBuild loading — the target framework, the Roslyn
Workspaces packages, and the `Microsoft.Build.*` packages — so that the process
runtime and the MSBuild assemblies it hosts share a major version. Mixing a
process runtime with MSBuild assemblies of a different major version is
prohibited: it is the mismatch that produced `AccessViolationException` crashes.

The indexer's own runtime alignment is independent of the SDK it drives. The
indexer SHALL be published self-contained, so it carries its net10 runtime and
runs unchanged regardless of which SDK major the target workspace pins. The SDK
used to evaluate the target is determined by the workspace's `global.json` and
provisioned into the shared toolchain cache before the indexer starts; the
indexer SHALL locate it through `DOTNET_ROOT` and SHALL NOT install it itself.

Self-contained does not imply single-file. There are two distributions and they
differ deliberately: the host artifact in `./build` is the single-file native
build governed by the single-file requirement, which self-extracts its BuildHost
on every start; the image payload is self-contained **multi-file**, laid out as a
directory so nothing is extracted at run time. Consolidating them would pay the
self-extraction cost on every containerized index for no benefit.

#### Scenario: The self-contained binary runs where no SDK major matches it

- **WHEN** kenn-dotnet runs against a workspace whose provisioned SDK is an older
  major than the indexer's own runtime
- **THEN** the binary launches (it carries its own runtime)
- **AND** indexing that workspace emits symbol frames without error

#### Scenario: A net10 target project is indexed

- **WHEN** the indexer indexes a project targeting `net10.0`
- **THEN** the C# package appears in the workspace index with its type symbols

#### Scenario: The SDK comes from the cache, not the image

- **WHEN** the indexer starts in a container carrying no .NET SDK
- **THEN** it resolves the SDK through `DOTNET_ROOT` from the shared toolchain
  cache
- **AND** `dotnet restore` binds package assemblies so package types resolve to
  their fully-qualified names

### Requirement: Out-of-process project evaluation

The indexer SHALL evaluate target projects through Roslyn's out-of-process
BuildHost child, so the sidecar process never hosts the target build. The
indexer SHALL sweep orphaned BuildHost child processes on exit.

#### Scenario: No in-process MSBuild mismatch crash

- **WHEN** the native single-file binary indexes a C# project repeatedly under
  the abort-regression stress on macOS arm64
- **THEN** every run completes without an `AccessViolationException` or a
  BuildHost non-zero exit

### Requirement: Single-file native distribution

The native self-contained build SHALL publish as a single file that carries the
out-of-process BuildHost, extracting all bundled content to disk at startup so
the BuildHost child can be located and launched.

#### Scenario: The lone binary indexes without co-located BuildHost files

- **WHEN** only the single-file binary is present (no sibling `BuildHost-netcore`
  directory) and it indexes a C# project
- **THEN** it self-extracts the BuildHost and indexing succeeds

