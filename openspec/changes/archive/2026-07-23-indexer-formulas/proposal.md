## Why

`brew install kennworx/tap/kenn` installs the CLI and nothing else. The three
indexers kenn actually ships — `kenn-ts`, `kenn-dotnet`, `kenn-swift` — are not
in the tarball, so a Homebrew user gets a tool that indexes none of the
languages kenn implements itself, and finds out at `kenn init` time.

Bundling all three into the single `kenn` formula is the obvious move and the
wrong one. Measured compressed, which is what a user downloads:

| binary | compressed | raw | built by |
|---|---:|---:|---|
| `kenn` | 5.4 MB | — | cargo |
| `kenn-dotnet` | 37.1 MB | 45 MB | `dotnet publish` self-contained |
| `kenn-ts` | 15.4 MB | 69 MB | `bun build --compile` |
| `kenn-swift` | 2.2 MB | 17 MB | `swift build` |

One fat formula is ~60 MB against 5.4 MB — an **11× download for every user**,
whichever languages they index, plus three foreign toolchains on every release
runner and a release that fails whenever any one of them breaks.

Note what the numbers do NOT say. `kenn-dotnet` is 68% of that weight on its
own, and `kenn-swift` compresses smaller than the CLI — so size alone would
justify splitting out C# and little else. The stronger argument is that the
three have genuinely different platform constraints (below), which a single
formula cannot express.

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

**`kenn-swift` needs a Swift toolchain on EVERY platform, macOS included.**
Measured with `otool -L` rather than assumed:

```
/usr/lib/swift/libswiftCore.dylib          ← macOS ships this
@rpath/libIndexStore.dylib                 ← it does NOT
rpath: /Applications/Xcode.app/Contents/Developer/Toolchains/…/usr/lib
```

`libIndexStore` is absent from `/usr/lib/swift`; it lives inside Xcode or the
Command Line Tools. A binary built on a machine with Xcode carries a **hardcoded
rpath into `/Applications/Xcode.app`** and fails on a machine that has only the
Command Line Tools — or Xcode elsewhere.

This is the same hazard `docker/kenn-swift/Dockerfile` already documents for
Linux ("kenn-swift LINKS the toolchain's index-store library, which the slim
base does not ship… exits 127, naming neither the library nor the reason"). It
applies on macOS too, and the first draft of this proposal missed it by
reasoning about `libswiftCore` alone.

So `kenn-swift` is not simply "the macOS-only one". It is the one whose formula
has an unresolved dependency question on every platform — bundle the library and
rewrite the rpath, or declare a toolchain dependency. See the design.

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
