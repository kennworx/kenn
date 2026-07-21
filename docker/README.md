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

**Nothing may write junk to stdout.** It is the JSONL wire. The entrypoint logs
diagnostics to stderr; the only thing it writes to stdout is one valid
`toolchain` frame naming the version it provisioned (the run summary reads it
back). `kenn-ts` forces the TypeScript compiler's `traceResolution` off for the
same reason a stray log line would corrupt the wire.

**The entrypoint `exec`s rather than spawns.** As PID 1 a spawn-and-wait parent
never reaps orphaned grandchildren, and indexers that shell out — `go list`,
`dotnet restore` — then fail while still reporting success.

## Offline / air-gapped use

Provisioning needs the network twice, and neither half can happen on an isolated
host: resolving a pin to a concrete artifact reads the vendor's release metadata
(that is where the download URL and checksum live), and fetching then downloads
the toolchain. So an air-gapped host cannot provision — it runs only against a
**pre-warmed** `kenn-toolchains` volume that already holds every version its
repositories pin. The volume is content-addressed by resolved version and
architecture:

```
kenn-toolchains/
  dotnet/9.0.316/   go/1.26.5/   rust/1.97.1/   python/3.14.6/   swift/6.3/
```

`kenn docker-cache ls` lists what is provisioned, with sizes; `clean --toolchain
<lang>[@<version>]` drops one. To populate a volume for an offline host:

1. On a networked machine of the **same architecture**, index repos pinning
   every version the offline host needs; confirm with `kenn docker-cache ls`.
2. Export: `docker run --rm -v kenn-toolchains:/v -v "$PWD":/out alpine \`
   `tar czf /out/kenn-toolchains.tgz -C /v .`
3. On the isolated host: `docker volume create kenn-toolchains`, then the same
   `tar xzf … -C /v` into it. Load the six images from a `docker save` tarball too.

**Swift needs one more thing.** Its toolchain is copied from the official
`swift:<tag>` image on the host at preflight, so that image must also be present
in the offline host's docker store (`docker save`/`load` it). Once the toolchain
is in the volume, preflight finds it and does no docker work.

## Verifying a change

Index a real repository and count symbols. `--version` passes on an image that
cannot index anything, which is exactly the failure being guarded against.
