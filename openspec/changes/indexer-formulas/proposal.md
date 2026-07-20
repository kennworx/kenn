## Why

`brew install kennworx/tap/kenn` installs the CLI and nothing else. The three
indexers kenn actually ships — `kenn-ts`, `kenn-dotnet`, `kenn-swift` — are not
in the tarball, so a Homebrew user gets a tool that indexes none of the
languages kenn implements itself, and finds out at `kenn init` time.

Bundling all three into the single `kenn` formula is the obvious move and the
wrong one. Measured:

| binary | size | built by |
|---|---:|---|
| `kenn` (release, xz) | 5 MB | cargo |
| `kenn-ts` | 69 MB | `bun build --compile` |
| `kenn-dotnet` | 45 MB | `dotnet publish` self-contained |
| `kenn-swift` | 17 MB | `swift build` |

The sidecars are **~26× the size of the CLI**. One fat formula charges every
user ~130 MB for languages they may not use, forces every release to install
three foreign toolchains on the runners, and makes the whole release fail when
any one of them breaks.

**One formula per indexer instead.** `brew install kennworx/tap/kenn` stays
5 MB; a C# user adds `kennworx/tap/kenn-dotnet`. Each formula carries its own
platform matrix and its own dependencies, which matters because they genuinely
differ (see `kenn-swift` below).

## What Changes

**Four formulas in the tap** rather than one:

- `kenn` — the CLI. Unchanged, still produced by `dist`.
- `kenn-ts` — TypeScript indexer.
- `kenn-dotnet` — C# indexer.
- `kenn-swift` — Swift indexer.

Each sidecar formula installs one binary under the name kenn looks up on
`PATH`, so an installed formula is picked up by `kenn init` with no config.

**A release path for non-cargo artifacts.** `dist` builds cargo packages; none
of the three sidecars is one. They need a workflow that builds each per
platform, attaches the archives to the release, and pushes a generated formula
to the tap — the same shape as `dist`'s homebrew publish, for artifacts `dist`
cannot see.

**`kenn-swift` is macOS-only initially.** It links `libswiftCore`, which macOS
ships and Linux does not — this is exactly why the Docker image is
`swift:*-noble-slim` based rather than plain noble. A Linux formula would need
`depends_on "swift"` pulling a full toolchain for a 17 MB binary. Deferred with
that reasoning recorded, not silently skipped.

**Third-party indexers stay out.** `rust-analyzer`, `scip-go` and
`scip-python` are separately maintained; kenn calls them rather than vendoring
them. The README now documents installing them, and `kenn init` already prints
the same install hints. Whether `kenn` should `depends_on "rust-analyzer"` is
deliberately a non-goal here — it would make a Rust toolchain a dependency of
using kenn on a Python project.

## Capabilities

### New Capabilities
- `indexer-distribution`: how kenn's own indexer binaries are built for
  release, attached to a release, and installed — including which platforms
  each supports and how an installed indexer is discovered.

### Modified Capabilities
- `workspace-init`: `kenn init`'s probe already reports a missing indexer as
  degraded with an install hint. The hints must name the Homebrew formula
  where one exists, so the reported fix matches how the user installed kenn.

## Impact

**New** — a release workflow for the sidecars, and formula generation for
three formulas. The generation must read checksums from the artifacts actually
uploaded rather than recomputing them; a previous hand-rolled attempt at this
had a guard that printed an error, rendered an empty `sha256`, and exited 0,
because `exit 1` inside a command substitution kills only the subshell.

**Build toolchains in CI** — bun, .NET SDK, and Swift, each only in the job
that needs it. A sidecar failing must not block the others or the `kenn`
release, which is the failure mode that already cost this repo two release
cycles when one target queued forever and another broke on an unrelated
network flake.

**Versioning** — the design must decide whether sidecars version in lockstep
with `kenn` or independently. They are in this repo and change with it, which
argues for lockstep; but a lockstep sidecar has to be rebuilt and republished
for every CLI patch release whether or not it changed.

**Not affected** — the Docker runtime. `runtime = "docker"` already delivers
every indexer including the third-party ones, and is unchanged by this.
