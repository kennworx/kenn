## 1. csproj version alignment

- [x] 1.1 `kenn-dotnet.csproj`: TFM `net8.0` → `net10.0`; Roslyn
      `Microsoft.CodeAnalysis.{CSharp.,}Workspaces.MSBuild` `4.7.0` → `5.6.0`;
      `Microsoft.Build.*` `17.11.4` → `18.8.2`; add `Microsoft.NET.StringTools`
      `18.8.2` with `ExcludeAssets="runtime" PrivateAssets="all"` (MSBL001);
      `Microsoft.Extensions.*` + `System.IO.Hashing` `8.x` → `10.0.1` (NU1605).
      Verify: `dotnet build -c Release` restores + compiles with zero errors and
      no C# source change.
- [x] 1.2 Pin `System.Security.Cryptography.Xml` `10.0.10` (transitive from
      Roslyn 5.6 flagged NU1903). Verify: no NU1903 in the build output.

## 2. Single-file native publish

- [x] 2.1 In the `RuntimeIdentifier != ''` PropertyGroup, keep
      `PublishSingleFile=true` and add `IncludeAllContentForSelfExtract=true`
      (+ `IncludeNativeLibrariesForSelfExtract`, `EnableCompressionInSingleFile`).
      Verify: `dotnet publish -r osx-arm64` produces one binary with **no**
      `NETSDK1236` warning; the binary COPIED ALONE (no sibling BuildHost dirs)
      indexes a net10 C# project and emits its type symbols.

## 3. Test project + build recipe

- [x] 3.1 `kenn-dotnet.tests.csproj` TFM `net8.0` → `net10.0` (it references the
      now-net10 lib). Verify: `just test-indexer-dotnet` builds + passes (75/75).
- [x] 3.2 `justfile` `build-indexer-dotnet`: the hardcoded `bin/Release/net8.0/…`
      publish path → `net10.0`. Verify: `just build-indexer-dotnet` copies a 44 MB
      `build/kenn-dotnet` that self-extracts + indexes.

## 4. Docker (framework-dependent — no Dockerfile logic change)

- [x] 4.1 Confirm the committed `docker/kenn-dotnet/Dockerfile` (framework-
      dependent on sdk:10) needs no change; update its stale net8 rationale
      comment. Verify: `just build-image kenn-dotnet` + reindex the polyglot
      fixture → the C# package is present (6/6).

## 5. Gates

- [x] 5.1 `just index-stability` (abort-regression stress, macOS arm64) — **0
      aborts / 8 runs**. The crash gate for the reverted AVE.
- [x] 5.2 `just test-indexer-dotnet` (xunit) — **75/75** on net10.0.
- [x] 5.3 `dotnet format --verify-no-changes` both projects — clean (no `.cs`
      changed).
