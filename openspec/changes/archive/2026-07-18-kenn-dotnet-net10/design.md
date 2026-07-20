# Design — kenn-dotnet net10 migration

## D1. Why net10 is safe now, and why net8 must move

Two separate things are often conflated; keep them apart.

1. **The historical AVE that made net8 look necessary is already fixed and is
   NOT a .NET-version issue.** It was root-caused (finding `fnd_b5f1b55b`,
   mutation-verified, guarded by `just index-stability`) to draining
   `dotnet restore`'s redirected pipes with `CopyToAsync`: an async completion
   landing after the buffer returned to the `ArrayPool` corrupts it and the
   process dies with an uncatchable `AccessViolationException`. The fix is
   blocking reads on a `LongRunning` thread (`SolutionLoader.StartBlockingDrain`);
   the BuildHost/MSBuild attribution the old csproj comment carried was a
   documented misattribution. This migration does **not** touch that drain, so
   the fix stands — `index-stability` stays 0 aborts.

2. **The concrete, current breakage is version, not crash.** On the sdk:10 base
   the net8 sidecar fails at two points I observed directly: it won't launch
   (`exit 150` — only the net10 runtime is present), and `MSBuildLocator` returns
   no instances (a net8 process can't load SDK 10's net10 MSBuild). The remedy is
   to align every layer to net10:

   | Layer | Was | Now |
   |---|---|---|
   | TFM | net8.0 | net10.0 |
   | Roslyn Workspaces | 4.7.0 | 5.6.0 |
   | Microsoft.Build.* | 17.11.4 | 18.8.2 |
   | Docker base runtime | net8 (absent on sdk:10) | net10 (sdk:10) |

A net10 process finds SDK 10 and hosts net10 MSBuild for the in-process
`SolutionFile.Parse`; project evaluation goes out-of-process (D2). Empirically
verified: `index-stability` 0/8 aborts, xunit 75/75, docker atlas 6/6.

## D2. Out-of-process BuildHost decouples the runtimes

Roslyn 5.6's `MSBuildWorkspace.Create()` runs project evaluation in an
out-of-process **BuildHost** child (`BuildHost-netcore/…BuildHost.dll`) on the
SDK's runtime, over an RPC pipe — the sidecar process never hosts the target
build. This is why a net10 sidecar can drive SDK 10's MSBuild safely, and why
the migration needs **no** "force in-process" hack (none is supported in 5.x
anyway). `MSBuildLocator.RegisterDefaults()` is kept — with a net10 process it
now finds SDK 10 — and still serves the in-process `SolutionFile.Parse` in
`SolutionLoader`. `BuildHostGuard` still sweeps orphaned BuildHost children.

## D3. Single-file preserved via IncludeAllContentForSelfExtract

The native distribution must stay a single binary (clean install). Plain
`PublishSingleFile` **excludes** the BuildHost's `.deps.json`/`.runtimeconfig.json`
(`NETSDK1236`), so the child can't launch. `IncludeAllContentForSelfExtract=true`
bundles all content into the one file and self-extracts it to `~/.net` (or
`DOTNET_BUNDLE_EXTRACT_BASE_DIR`) at startup, where Roslyn locates and launches
the BuildHost. Verified: the **lone** 44 MB binary, with no co-located BuildHost
dirs, indexes a net10 project correctly.

Tradeoff: first run extracts ~30 MB to `~/.net`; subsequent runs reuse it.

## D4. Two publish shapes, one csproj

- **Docker** (`RuntimeIdentifier` empty → framework-dependent): the BuildHost
  ships as loose files in the publish output; the net10 base supplies the
  runtime. No single-file machinery. **Dockerfile unchanged.**
- **Native** (`-r <rid>` → self-contained single-file): the
  `Condition="'$(RuntimeIdentifier)' != ''"` PropertyGroup turns on
  `PublishSingleFile` + `IncludeAllContentForSelfExtract`.

## D5. Validation gates (the crash history demands them)

- `just index-stability` — the abort-regression stress on macOS arm64; guards the
  pipe-drain AVE (D1). Must stay 0 aborts (confirms the migration didn't disturb
  the blocking-read fix).
- `just test-indexer-dotnet` — xunit suite (unchanged C#, new Roslyn).
- Docker: rebuild image → reindex the polyglot fixture → C# package present (6/6).
- `dotnet format --verify-no-changes`.

## D6. Alternatives rejected

- **Add net8 SDK to the sdk:10 image (in-process net8 MSBuild):** works but caps
  indexing at net8 targets (net8 MSBuild can't build net9/10 projects). Rejected —
  the point of the migration is modern-target coverage.
- **Drop single-file (self-contained directory):** avoids `NETSDK1236` but breaks
  the single-binary install UX. Rejected in favor of D3.
- **Force in-process MSBuild on 5.x:** no supported toggle; the out-of-process
  BuildHost is the built-in default.
