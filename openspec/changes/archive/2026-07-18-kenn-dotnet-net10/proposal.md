# Migrate kenn-dotnet to .NET 10

## Why

The kenn-dotnet sidecar targets **net8.0** with **Roslyn 4.7** and an in-process
MSBuild model (`MSBuildLocator.RegisterDefaults()`). This breaks against a
current `mcr.microsoft.com/dotnet/sdk:10.0` base — the very base the docker
indexer image (`docker-indexer-runtime`) ships on:

- The framework-dependent net8 binary won't launch: the sdk:10 image carries
  only the net10 shared runtime (`exit 150`).
- Even with the net8 runtime added, `MSBuildLocator` returns **no instances** —
  a net8 process cannot load SDK 10's net10 MSBuild in-process.

net8 was chosen after an earlier net10 attempt hit intermittent
`AccessViolationException` crashes on macOS. That AVE was later root-caused (see
the `just index-stability` finding) to draining `dotnet restore`'s pipes with
`CopyToAsync` — **not** the .NET version — and is **already fixed** with blocking
reads (`SolutionLoader.StartBlockingDrain`), guarded by `index-stability`. So
net10 is safe now. The concrete reason to move: net8 no longer runs on the
sdk:10 base — the framework-dependent binary fails to launch (`exit 150`, the
image carries only the net10 runtime) and `MSBuildLocator` finds no instances (a
net8 process can't load SDK 10's net10 MSBuild). C# dropped out of the polyglot
atlas (5/6).

## What Changes

Move the whole stack to the **net10 generation** so the runtime and MSBuild
versions match, and rely on Roslyn 5.6's **out-of-process BuildHost** (project
evaluation runs in a child process on the SDK runtime — decoupled from the
sidecar's runtime, so the in-process mismatch crash cannot occur):

- `kenn-dotnet.csproj`: TFM `net8.0` → `net10.0`; Roslyn Workspaces `4.7.0` →
  `5.6.0`; `Microsoft.Build.*` `17.11.4` → `18.8.2` (+ `Microsoft.NET.StringTools`
  runtime-exclusion); `Microsoft.Extensions.*` / `System.IO.Hashing` → `10.0.1`.
- Native self-contained **single-file** is preserved by adding
  `IncludeAllContentForSelfExtract=true` — the out-of-process BuildHost rides
  inside the one binary and self-extracts to `~/.net` at runtime.
- The docker image is **framework-dependent** on sdk:10 and needs **no
  Dockerfile change** (a net10 binary runs natively on the net10 base).
- `kenn-dotnet.tests` TFM → `net10.0`; the `build-indexer-dotnet` recipe's
  hardcoded `net8.0` publish path → `net10.0`.

**No C# source changes** — the `MSBuildWorkspace` API is stable 4.7 → 5.6.

## Impact

- Affected: `indexers/kenn-dotnet/**`, `docker/kenn-dotnet/Dockerfile` (comment),
  `justfile` (`build-indexer-dotnet`). Restores C# to the docker atlas (6/6).
- Behavior: the native binary self-extracts ~30 MB to `~/.net` on first run
  (subsequent runs reuse it). The docker image gains net9/net10 target coverage.
- Non-goals: no change to the wire format, the indexer logic, or the other five
  language indexers.
