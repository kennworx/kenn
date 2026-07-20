# Indexer images

One image per language, built by `docker buildx bake -f docker/bake.hcl`.

## The rule

**What the workspace pins is provisioned. What the indexer needs is payload.**

A repository declares its toolchain — `global.json`, `rust-toolchain.toml`,
`go.mod`, `.python-version`, `Package.swift` — and that version is fetched at
index time into a shared cache volume. Baking one instead means the image and
the repository can disagree, and when they do the index does not fail: it
returns nothing.

The indexer itself is the opposite. No repository has an opinion about which
build of `rust-analyzer`, `scip-go`, `scip-python` or `node` we run, so those
ship in the image.

| image | payload | provisioned |
|---|---|---|
| csharp | kenn-dotnet | .NET SDK |
| go | scip-go | Go toolchain |
| rust | rust-analyzer | rustc, cargo, rust-src |
| python | scip-python, node | CPython |
| swift | kenn-swift | Swift toolchain (see below) |
| typescript | kenn-ts (bun embedded) | — |

## Shape

Every image is `ubuntu:noble` or a noble-based vendor image, and runs
`kenn-toolchain` as its ENTRYPOINT. That binary reads the pin, provisions into
`/kenn-toolchains`, puts the toolchain on `PATH`, and `exec`s the real indexer.

The entrypoint is a shared bake target, built once per platform rather than once
per image.

**Swift is the exception.** Its vendor publishes no verifiable download — a URL
and a detached PGP signature, no checksum in machine-readable form — so instead
of fetching, kenn copies the toolchain out of the official `swift:<tag>` image,
where content is addressed by digest and every layer is verified on pull. That
needs a docker daemon, so it happens on the HOST during preflight; the entrypoint
finds the toolchain already in the volume and only does the PATH-and-exec half.

**One distro everywhere is deliberate.** Alpine saved ~65 MB per image and cost
far more than that in libc mismatches: vendors' default Linux artifacts are
glibc, and a musl/glibc mismatch shows up as a binary that exists and will not
exec, naming neither the file nor the reason.

**Noble specifically**, because the swift image is noble-based and that is the
one base we do not get to choose — matching it keeps a single glibc floor (2.39)
rather than a debian/ubuntu split.

Builder stages are exempt and several are still bookworm (`rust:1-slim-bookworm`
for the entrypoint — no official `rust:*-noble` exists — plus the go and node
sidecars). That direction is the safe one: a 2.36 build runs on a 2.39 runtime,
never the reverse. A builder may drop below the runtime floor, never above it.

## Two constraints that are easy to break

**Nothing may write to stdout.** It is the JSONL wire. The entrypoint logs to
stderr; `kenn-ts` forces the TypeScript compiler's `traceResolution` off for the
same reason.

**The entrypoint `exec`s rather than spawns.** As PID 1 a spawn-and-wait parent
never reaps orphaned grandchildren, and indexers that shell out — `go list`,
`dotnet restore` — then fail while still reporting success.

## Verifying a change

Index a real repository and count symbols. `--version` passes on an image that
cannot index anything, which is exactly the failure being guarded against.
