## Context

Indexer images currently bake a language toolchain chosen at image-build time.
The target repo independently declares the version it wants (`global.json`,
`rust-toolchain.toml`, `go.mod`, `Package.swift`, `.python-version`). When the
two disagree, the failure is silent: hostfxr resolves no SDK, MSBuildLocator
reports "no usable MSBuild instance", and the run exits 0 having indexed nothing.

Measured composition — the toolchain is nearly the whole image, and our own code
is a rounding error:

```
image        total     toolchain bulk        ours
swift       2.55 GB    1.75 GB  (69%)        ~40 MB
csharp      1.03 GB    642 MB   (62%)        28.5 MB  (4%)
rust         958 MB    584 MB   (61%)        —  (external tool)
go           846 MB    481 MB   (57%)        —  (external tool)
python       419 MB    217 MB   (52%)        ~5 MB
typescript   175 MB    —                     93 MB    (already thin)
```

Shipping N versions per language does not converge: three .NET SDKs cost 1.88 GB
and still break on the fourth pin.

Once the toolchain leaves, what remains is a single binary. Every one of them
links the same three libraries (`ld-musl`, `libstdc++`, `libgcc_s`;
rust-analyzer swaps in `libmimalloc-secure`) — about 3 MB of `.so` files:

```
kenn-ts        98 MB     rust-analyzer  20.9 MB
kenn-swift   38.8 MB     scip-go        17.3 MB
kenn-dotnet  28.5 MB     scip-python      31 MB (JS + 18 MB typeshed data)
```

Five images totalling 5.56 GB reduce to ~204 MB of payload. At that size the
image is no longer an OS with a program in it; it is a program.

## Goals / Non-Goals

**Goals:**
- A workspace's declared toolchain version is what gets used, for every language
  that declares one.
- A version mismatch fails **loudly**, never as a zero-symbol success.
- Toolchains are provisioned once per machine and shared across workspaces.
- The same resolution applies to `runtime = "local"`, not just docker.
- Images carry our sidecar and nothing else the target repo could have an opinion
  about.

**Non-Goals:**
- Reimplementing toolchain managers. Each language has an official relocatable
  installer; we drive it, we do not replace it.
- Provisioning *dependencies* — the existing per-language dependency cache
  volumes (e.g. `NUGET_PACKAGES`) are unchanged and orthogonal.
- Restructuring the TypeScript image beyond aligning its base.
- Supporting arbitrary/nightly toolchain channels in the first pass.

## Decisions

### Provisioning runs in the container, in a kenn-authored entrypoint

Downloads happen inside docker, and kenn calls `docker` only to build images — it
does not orchestrate provisioning from the host. So every indexer image runs
`kenn-toolchain` as its ENTRYPOINT: it resolves the workspace's pin, provisions
into the mounted cache volume if the toolchain is absent, then `exec`s the real
indexer.

The obvious objection to in-container provisioning is that **three of six
languages have no code of ours to hook** — rust, go and python run third-party
binaries (`rust-analyzer`, `scip-go`, `scip-python`) that will never read a pin
file. A kenn entrypoint *in front of* the third-party binary answers that: the
provisioning step is the same program in all six images, and the indexer behind
it is whatever that language needs.

**Why a Rust binary and not a shell script.** The work is not "download a file" —
it is .NET's `rollForward` semantics, Rust's `[renames]` table, Go's `toolchain`
vs `go` line, and three different metadata formats. That is real logic, and in a
shell script it would be untestable. In Rust it has unit tests against recorded
fixtures and mutation checks on the branches that matter.

**What this costs:** we read each vendor's release metadata ourselves rather than
delegating to `dotnet-install.sh` / `rustup`, which makes verification our
responsibility rather than the installer's — see below.

### Downloaded artifacts are verified against a published checksum

TLS authenticates the host we fetched from, not the bytes we received. Every
vendor publishes a hash alongside the artifact in the same release metadata that
gives us its URL; the provisioner SHALL fetch both and verify before unpacking.
An earlier draft of the fetch path omitted this entirely — 600 MB unpacked into a
shared cache on the strength of a TLS handshake. Absent or mismatched, the
install fails; it is never "probably fine".

### Docker builds the images, and the build does the work

An earlier draft proposed assembling OCI layers directly in Rust — no daemon,
byte-reproducible digests, multi-arch without QEMU. That was rejected, and the
rejection is right for a reason worth writing down: **the payoff was real but the
liability was ours.** A hand-rolled builder is code we maintain forever, and a
bug in it breaks publishing for all six languages simultaneously. Reproducible
digests are a genuinely better security property than "whatever CI produced that
Tuesday", but they are not worth owning a build system to get.

It also quietly depended on images having no `apk add` step — which stopped being
true the moment `git` had to ship (scip-python spawns it, and we cannot patch a
third-party indexer). Sourcing `git` as prebuilt files for two architectures was
the hidden cost, and it disappears with `docker build`.

**As much as possible happens inside the build.** `kenn-toolchain` is compiled in
a builder stage for the image's own platform rather than cross-compiled on the
host and copied in. The host needs nothing but Docker: no aarch64-musl toolchain,
no per-developer setup, and a build that behaves the same in CI as on a laptop.

### The base is chosen per image, by what the vendor actually ships

Alpine was the right default when images BAKED toolchains — it saved 300–600 MB
per image. That premise is gone: nothing is baked, so the base is now a small
fraction of a small image (the C# image is 187 MB, of which alpine is ~8 MB).

What replaced size as the deciding factor is **libc**. Every vendor's default
Linux artifact is glibc, and provisioning one into an alpine container produces a
binary that exists on disk and fails to exec with a bare `not found` — naming
neither the file nor the reason. Measured on .NET before the RID was corrected.

So the rule is: **alpine where the vendor publishes a first-class musl build,
glibc where it does not.** Do not force alpine where it costs more than it saves.

| language | musl available | base |
|---|---|---|
| csharp | `linux-musl-*` SDK tarballs, proven working | alpine |
| rust | toolchain has musl, BUT rust-analyzer publishes gnu-only binaries and the alpine package depends on rust-src (drags the toolchain back in, 938 MB) | glibc |
| go | binaries are static, BUT cgo targets compile against the image libc | glibc |
| typescript | bun-compiled with a musl target | alpine |
| python | python-build-standalone has musl assets, but **Node does not** — official builds are glibc-only and musl exists only on unofficial-builds.nodejs.org | glibc |
| swift | swift.org publishes Ubuntu/RHEL only; there is no musl toolchain at all | glibc |

The glibc bases cost ~40 MB more. That is worth paying to consume a vendor's
supported artifact instead of a community rebuild whose retention and
architecture coverage we do not control.

### Containers still need a CA bundle

`dotnet restore` reaches nuget.org, `swift build` fetches packages, cargo may hit
crates.io. Scratch has no trust store, and the failure is an opaque TLS error.
~200 KB of `ca-certificates` ships in every image deliberately.

### The payload is what the indexer executes, not what we wish it executed

An earlier draft required that no image depend on a shell, package manager, or
`git`. That was purity, not engineering, and it was unsatisfiable: scip-python
shells out to `pip list` and reads the project version from `git` at index time
(recorded in the image's own comment), and it is third-party — we cannot patch
its process spawns the way we could relocate kenn-ts's `git worktree list` probe.

So the rule is narrower and true: **no toolchain, no installer.** Auxiliary
executables an indexer actually spawns are part of its payload and ship in the
image. Establish that set by observing what the indexer runs, not by assuming it
is self-contained.

### The toolchain volume is CLI-manageable, and reclaimable per toolchain

It is the largest thing kenn puts on disk — roughly 600 MB per provisioned
toolchain — and it is bound to no directory, so `--orphans` will never reap it.
Left implicit it would grow without bound and without visibility. `ls` therefore
reports each provisioned toolchain by language, resolved version, and size, and
`clean` can drop one toolchain (`--toolchain <lang>[@<version>]`) without razing
the volume and forcing every other workspace to refetch.

### Staleness rides on the pin file, not on a new signature

A toolchain change must force a reindex, but it needs no new mechanism: pin files
(`global.json`, `rust-toolchain.toml`, `go.mod`, `Package.swift`) are tracked
content, and the staleness key already hashes tracked-modified files. Editing one
changes the key. The index additionally records the resolved toolchain version and
reports it in the run summary, so a change is attributable rather than silent —
but that record is diagnostic, not a second staleness input.

### Node is a toolchain; scip-python is not bun-compiled

scip-python is a Node application. A spike established that `bun build --compile`
does not work on it: it is a webpack build whose runtime resolves `vendor.js` and
`pyright-internal.js` through a computed `require` that no bundler sees
statically. Requiring the chunks explicitly does embed them, but bun's compiled
binary resolves internal requires through its own resolver, so shimming
`Module._load` has no effect. Making it work means patching the published `dist/`
or re-running webpack against scip-python's source — maintaining a build of a
third-party tool.

The payoff would not justify it anyway. Bun's runtime floor is 63 MB native /
89 MB musl against node's 126 MB binary: it swaps one JS runtime for another,
saving ~37 MB, and the 18 MB typeshed data ships loose either way.

Provisioning node into the toolchain volume is both smaller and simpler — the
python image carries only scip-python's 31 MB `dist/`, with node and python3
shared from the volume. Python stops being a special case.

### Cache layout is content-addressed by resolved version, not by pin

```
<cache-volume>/
  dotnet/9.0.308/
  dotnet/8.0.404/
  swift/5.10.1/
  rust/1.83.0/
```

A pin (`"9.0.308"` with `rollForward: latestMinor`) resolves to a concrete
version first; the concrete version is the cache key. Two repos pinning
differently but resolving identically share one install.

### Provisioning is guarded by a lockfile, and is idempotent

Multiple kenn runs can target the same toolchain concurrently against one shared
volume. Install into a temp dir and atomically rename into place, holding an
exclusive lock on the destination key. An interrupted install must never leave a
half-populated directory that a later run mistakes for a complete one.

### Go is not implemented — it is configured

Go 1.21+ already resolves and downloads the toolchain named in `go.mod` when
`GOTOOLCHAIN=auto`. Writing our own resolver for Go would be strictly worse than
the one shipped in the tool. We set the env var and mount the module cache.

### `runtime = "local"` provisions nothing

Provisioning lives in the indexer container, so there is none without one. A
local run uses whatever toolchain the developer has installed, exactly as today,
and a missing one is reported rather than fixed.

This is a deliberate narrowing from an earlier draft that had kenn installing
toolchains into `~/.kenn/toolchains/` under an opt-in. That version had to invent
a consent rule, a second install location, and a precedence rule against
`kenn init --docker`. None of it is needed now, and writing toolchains onto a
developer's machine was the part of this change most likely to surprise someone.

### A provisioning run emits progress on the wire

A first-time toolchain fetch is ~200 MB and can take minutes. The sidecar emits
a status frame before and after. This is the same failure mode the meta-frame
flush just fixed: a silent producer during a long pre-index phase is
indistinguishable from a hung one.

### Resolution failure is fatal and named

If the pin cannot be resolved or installed, the run **fails** with the pin quoted
and its source file named. It does not fall back to whatever toolchain happens to
be present — that fallback is precisely the bug this change exists to remove.

## Risks / Trade-offs

- **New network dependency in the index path.** First index of an unseen version
  downloads a toolchain. Offline and air-gapped use now requires a pre-warmed
  cache volume; this must be documented and a warm-cache command considered.
- **Runtime failures replace build-time ones.** A broken image used to be caught
  in CI. A broken *install* surfaces mid-run on a user's machine. Mitigated by
  the loud-failure rule, but the surface is genuinely larger — and it lands on a
  pipeline that just had two silent frame-loss bugs. Verification must index a
  real fixture per language, not run `--version`.
- **Mutable shared state.** The cache volume is written by concurrent runs. The
  lock plus atomic-rename is the mitigation; it is also the most likely place for
  a subtle bug.
- **Version drift.** Installing by channel (`--channel 9.0`) yields a different
  patch over time, so two machines can resolve the same pin differently. Pinning
  the exact resolved version in the cache key makes this visible but does not
  prevent it.
- **First-run latency is worse; steady state is better.** A user indexing one
  repo of one language pays a download they previously got in the image pull.
  The win is that they pay it once per version across every workspace, and only
  for versions they actually use.
- **Five languages, five installers.** The pattern is shared but each pin grammar
  and installer is its own integration with its own failure modes. Sequencing
  (C# first, where the pin reader and a proven failure already exist; then Swift,
  the biggest payoff) keeps the blast radius per step small.
- **Digests stay unreproducible.** Sticking with `docker build` means an
  `IMG_*` pin continues to mean "whatever CI produced that day" rather than "this
  exact payload". Accepted deliberately: owning a builder to fix it costs more
  than the property is worth here.
- **A missing runtime dependency surfaces at index time, not build time.** A
  forgotten CA bundle is an opaque TLS failure inside `dotnet restore`, not a
  build error. Every image must be verified by indexing a real fixture —
  `--version` passes on an image that cannot reach a package registry.
- **The entrypoint is now in the path of every containerized index.** A bug in it
  breaks all six languages at once, where previously each image failed
  independently. It is small and heavily tested for that reason, and it execs the
  real indexer rather than wrapping its I/O, so it cannot corrupt the wire.

## Measured results (task 5.4)

Measured on arm64 / macOS Docker Desktop. Image sizes are the per-image
`docker images` figure — do NOT sum them: the `ubuntu:noble` base is shared but
counted once per image. Timings evict a toolchain with
`kenn docker-cache clean --toolchain <lang>`, then run a cold `kenn index
--force` (re-provisions) and a warm one (cache hit) on a real cloned repo.

**Image size, fat → thin.** The one before/after pair measured directly is C#:
**1.03 GB → 408 MB**. The old five images totalled **5.56 GB** carrying only
**~204 MB** of payload; the rest was bundled toolchain. Thin images now:

| image | csharp | typescript | go | python | rust | swift |
|---|---|---|---|---|---|---|
| size | 408 MB | 405 MB | 538 MB | 520 MB | 598 MB | 877 MB |

**Payload per image** (indexer + its runtime deps, the part that is not
toolchain): kenn-ts 98 MB, kenn-swift 38.8 MB, kenn-dotnet 28.5 MB,
rust-analyzer 20.9 MB, scip-go 17.3 MB, scip-python 31 MB.

**Provisioned toolchain size** (in the shared volume, provisioned ONCE per
version across every workspace on the machine): dotnet 604 MB (9.0.316) /
849 MB (10.0.302), rust 600 MB, go 264 MB, python 94 MB, swift 2.4 GB.

**First-index (cold) vs warm-cache, measured:**

| language | repo | toolchain | cold | warm | provision Δ |
|---|---|---|---|---|---|
| go | google/uuid | go 264 MB | 67 s | 8 s | ~59 s |
| csharp | serilog | dotnet 849 MB | 384 s | 13 s | ~371 s |

The trade holds as designed. The first index of an unseen toolchain version pays
a one-time download+unpack that scales with toolchain size — and .NET's
many-file SDK unpacks slower per byte (~2.3 MB/s effective) than Go's single
tarball (~4.5 MB/s). Every subsequent index of ANY workspace on that version is
warm: **8–13 s**. The bundled-SDK image re-fetched its whole toolchain on every
image pull; this pays once per version per machine, only for versions actually
used.

**Spawned executables, observed (task 4.2).** `strace -f -e trace=execve` on a
wrapper image built FROM each thin image (identical payload + strace), indexing
a real repo. Each indexer `exec`s only its provisioned toolchain driver plus
payload the image already ships:

| indexer | exec'd (beyond loader/libc) |
|---|---|
| scip-python | `git rev-parse`, `pip3 list`, `node` (runs scip-python), `python3`, `sh` |
| kenn-ts | `git worktree list` |
| scip-go | `git` (remote / rev-parse / tag), `go list` |
| rust-analyzer | `cargo` (check / metadata), `rustc` (+ build scripts → `cc`) |
| kenn-dotnet | `dotnet` (BuildHost, MSBuild, `restore`), `sh` |
| kenn-swift | `git` (describe / rev-parse / status), `swift build`, `swiftc` |

`git` is payload in every image; `sh` is in the base; the language driver
(`go`/`cargo`/`rustc`/`dotnet`/`swift(c)`/`node`/`pip3`/`python3`) comes from the
provisioned toolchain. Nothing spawned is absent — which is why 4.3 indexed all
six. This is the empirical form of "the payload is what the indexer executes":
`git` earns its place in every image, and rust/go keep `gcc`+`libc6-dev` because
build scripts and cgo compile native code (seen as the `rustc … build_script`
execs).
