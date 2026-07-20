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

#### Scenario: Framework-dependent binary runs on the SDK-10 base

- **WHEN** kenn-dotnet is published framework-dependent and run on a
  `mcr.microsoft.com/dotnet/sdk:10.0` image
- **THEN** the binary launches (the net10 shared runtime is present)
- **AND** indexing a C# project emits symbol frames without error

#### Scenario: A net10 target project is indexed

- **WHEN** the docker image indexes a project targeting `net10.0`
- **THEN** the C# package appears in the workspace index with its type symbols

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

