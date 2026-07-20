## Context

`dist` owns the release: it builds the cargo package, creates the GitHub
release, uploads archives, and pushes `Formula/kenn.rb` to the tap. Its
workflow is **generated** from `dist-workspace.toml` and must not be
hand-edited — CI regenerates it and a manual change is silently overwritten.

The three sidecars are outside all of that. None is a cargo package: `kenn-ts`
is `bun build --compile`, `kenn-dotnet` is `dotnet publish --self-contained`,
`kenn-swift` is `swift build`. They are built today only by `just install`, on
the developer's own machine, for whichever toolchains happen to be present.

So the problem is not "add three formulas" — it is "produce release artifacts
for things `dist` cannot see, without taking over the release `dist` owns."

## Goals / Non-Goals

**Goals:**
- `brew install kennworx/tap/kenn` stays small; indexers are opt-in per language.
- An installed sidecar is discovered by `kenn init` with no configuration.
- A sidecar build failure never blocks the `kenn` release or the other sidecars.
- Formula checksums come from the artifacts actually uploaded.

**Non-Goals:**
- Bundling third-party indexers (`rust-analyzer`, `scip-go`, `scip-python`).
- `depends_on "rust-analyzer"` on the `kenn` formula — that would make a Rust
  toolchain a dependency of using kenn on a Python project.
- Linux `kenn-swift` (see D3).
- Changing the Docker runtime, which already delivers every indexer.

## Decisions

### D1 — One formula per indexer, named after the binary

`kenn-ts`, `kenn-dotnet`, `kenn-swift`. Each installs exactly one executable
under the name kenn probes for on `PATH`, so installing the formula is
sufficient — no config, no post-install step.

Naming the formula after the binary rather than the language (`kenn-csharp`)
keeps the install command and the `PATH` lookup the same string, which is what
a user has to reason about when `kenn init` says a command is not runnable.

### D2 — Sidecars version in lockstep with the CLI

One tag drives everything. The sidecars live in this repo and change with it,
and independent versioning would mean a compatibility matrix between CLI and
sidecar wire formats that nothing currently needs — the JSONL protocol is
internal and both sides ship together.

The cost is real and accepted: a CLI-only patch release rebuilds and
republishes all three sidecars. That is CI time, not user cost, and it keeps
"which sidecar goes with which kenn" from ever being a question.

### D3 — `kenn-swift` has a toolchain dependency on EVERY platform

Measured, not assumed — `otool -L` on a release build:

```
/usr/lib/swift/libswiftCore.dylib      ← macOS ships this
@rpath/libIndexStore.dylib             ← it does NOT
rpath includes /Applications/Xcode.app/Contents/Developer/…/usr/lib
```

`libIndexStore` is not part of the OS on either platform. On macOS it lives in
Xcode or the Command Line Tools; on Linux the Docker image copies
`libIndexStore.so` out of the vendor image for exactly this reason. A binary
built where Xcode is installed bakes an absolute `/Applications/Xcode.app` rpath
and breaks on a machine with only the Command Line Tools.

The first version of this design said "macOS ships the Swift runtime, so macOS
is the easy case". That is true of `libswiftCore` and false of the library that
actually matters here.

Three ways out, to be decided in implementation:

1. **Vendor `libIndexStore` into the formula** and rewrite the rpath with
   `install_name_tool` so it loads from the Cellar. Self-contained, and the
   formula owns a copy of a toolchain library — including its licensing and its
   version drift against the user's Swift.
2. **`depends_on xcode:`** and rely on the toolchain path. Honest about the
   requirement, but Homebrew's `xcode` dependency does not accept a
   Command-Line-Tools-only install, so it demands the full Xcode from users who
   may only need the CLT.
3. **Build against the Command Line Tools path** so the rpath targets
   `/Library/Developer/CommandLineTools/usr/lib`, which is the more common
   install. Smallest change; still a hard dependency, just a cheaper one.

Option 1 is the only one that makes `brew install kenn-swift` work with no
further setup, and is the default recommendation — but the licensing of
redistributing a toolchain library has to be checked before committing to it.
That check is a task, not an assumption.

Linux stays out of scope regardless: it needs the runtime *and* the index-store
library, and `runtime = "docker"` already delivers both.

`kenn-ts` and `kenn-dotnet` are genuinely self-contained and ship for both
platforms — `kenn-dotnet` is published `--self-contained`, and `kenn-ts` is a
`bun --compile` binary with the runtime inside it.

### D4 — A separate workflow on the same tag, uploading to the same release

The sidecar workflow triggers on the same `v*` tag as `dist`, builds each
sidecar in its own job, and uploads to the release `dist` created.

This means it must tolerate the release not existing yet — `dist` may still be
building. Poll for the release with a bounded wait rather than assuming
ordering; cross-workflow ordering is not expressible in Actions, and assuming
it is produces a race that passes locally and fails on a slow runner.

**Open, and to be settled first (task 1):** `dist` has an `extra-artifacts`
mechanism that may be able to build and attach these itself, which would remove
the second workflow and the race entirely. I have NOT verified whether it
supports per-platform artifacts — it appears to run in the global job, which
would be wrong for platform-specific binaries. Verify before building the
separate workflow; if it works, prefer it.

### D5 — Every sidecar job is independent and non-blocking

`fail-fast: false`, and no job depends on another sidecar. A missing Swift
toolchain on a runner must cost the Swift formula, not the C# one.

This repo has already lost two release cycles to the opposite arrangement: one
target queued forever on a retired runner image and took the whole matrix with
it, and a transient network error in one publish step skipped every downstream
job including the formula push.

### D6 — Formulas are generated, and checksums are READ not recomputed

Each formula is rendered from the uploaded archives' `.sha256` files, so a
formula can only ever describe artifacts that were actually published.

The validation must run **before** the template is expanded, in the parent
shell. A previous hand-rolled attempt in this repo put the check inside a
helper called from a heredoc command substitution, where `exit 1` kills only
the subshell: it printed the error, rendered an empty `sha256`, and exited 0 —
a formula installing nothing, from a release reporting success.

## Risks / Trade-offs

**Three foreign toolchains in CI.** bun, the .NET SDK, and Swift each have to
be installed on release runners. That is three more things that can break a
release, mitigated by D5 but not eliminated.

**A partial release becomes normal.** With independent jobs, `kenn` can publish
while `kenn-swift` fails. That is the correct trade — but it means "the release
succeeded" no longer implies every formula updated, and the tap can hold a
`kenn-swift` pinned to an older version than `kenn`. Given D2's lockstep
versioning, a user could install mismatched versions. Acceptable only because
the JSONL protocol is stable across a patch; if that stops being true, D2 has
to grow a compatibility check.

**Homebrew formula review.** These are tap formulas, not homebrew-core, so
there is no external review — nothing catches a malformed formula except
installing it. The verification for this change has to be an actual
`brew install` of each formula, not a successful publish.

**macOS code signing.** `just install` re-signs binaries after copying because
`cp` invalidates the ad-hoc signature and AMFI then SIGKILLs them at exec.
Homebrew's own install path may hit the same thing; if it does, the formula
needs a `codesign` step and the failure will look like an unexplained crash
rather than a signing error.
