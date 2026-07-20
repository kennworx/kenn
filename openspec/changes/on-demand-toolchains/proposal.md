## Why

Baking a language toolchain into an indexer image hardcodes a version choice the
target repository has already made for itself. When they disagree the index does
not fail loudly — it returns nothing. Measured on a real 146-project solution:
a repo pinning `global.json` SDK `9.0.308` indexed by the published SDK-10 image
produced **0 files, exit 0**. With the pinned major present: 952 files, 15,971
symbols, 64,763 edges.

Shipping every plausible version is not a fix, it is a tax. Three .NET SDKs cost
1.88 GB — and still fail the day a repo pins a fourth. The toolchain is also
almost the entire image: our own indexer is 28.5 MB of a 1.03 GB C# image (4%);
the other 96% is a copy of Microsoft's SDK that we guessed at.

Every language we index already declares the version it wants, in a file we can
read. We should read it and provision that toolchain on demand into a cache
shared across workspaces, instead of guessing at build time and shipping the
guess.

## What Changes

- **Every** indexer image is reworked to carry its payload and no toolchain: the
  tool binary, the provisioning entrypoint, `git` and CA certificates, on a small
  base. Measured payloads — kenn-ts 98 MB, kenn-swift 38.8 MB, kenn-dotnet
  28.5 MB, rust-analyzer 20.9 MB, scip-go 17.3 MB, scip-python 31 MB. **Five
  images totalling 5.56 GB carry ~204 MB of payload**; the rest is toolchain
  that moves to the shared cache.
- Images keep being built by **`docker build` from a Dockerfile**. No bespoke OCI
  assembler — it would be ours to maintain, and one bug in it would break
  publishing for all six languages at once.
- **Everything that can happen inside the build, does.** `kenn-toolchain` is
  compiled in a Dockerfile builder stage for the image's own platform rather than
  cross-compiled on the host, so the host needs nothing but Docker and no
  aarch64-musl toolchain has to exist on it.
- A **toolchain cache volume**, shared across every workspace on the machine,
  holds provisioned toolchains keyed by language and version. Extends the
  existing per-language dependency-cache volume mechanism.
- Each language driver gains a **pin reader** (its own declaration file) and an
  **installer** (its own official, relocatable toolchain manager):

  | language | pin file | installer |
  |---|---|---|
  | csharp | `global.json` | .NET SDK tarball |
  | swift | `Package.swift` (`swift-tools-version`) | swift.org tarball |
  | rust | `rust-toolchain.toml` | rustup |
  | go | `go.mod` (`go` / `toolchain`) | `GOTOOLCHAIN=auto` (native) |
  | python | `.python-version`, `requires-python` | `uv python install` |

- **Provisioning happens inside the container**, in a new kenn-authored
  entrypoint (`kenn-toolchain`) that every image runs: it resolves the pin,
  provisions into the mounted cache, then execs the real indexer. kenn calls
  `docker` only to build images and run indexers — it never orchestrates a
  download. An entrypoint *in front of* the indexer is what makes this uniform
  across the three languages that run third-party binaries and have no kenn code
  to hook. Keeping the logic in a Rust binary rather than a shell script is what makes
  rollForward semantics and metadata parsing testable, and keeps the image from
  needing an installer script.
- **Artifacts are checksum-verified** against the vendor's published hash before
  unpacking. TLS authenticates the server, not the bytes.
- **Node is PAYLOAD, not a toolchain.** scip-python is a Node application, so
  node is what the INDEXER needs, not what the workspace pins — no repository
  has an opinion about which Node our indexer runs on. It ships in the image
  alongside scip-python; only Python is provisioned.
- **`runtime = "local"` provisions nothing.** Provisioning lives in the container,
  so a local run uses the toolchain already on the machine and reports a missing
  one rather than installing over it.
- **`kenn docker-cache` gains the toolchain volume.** It is the largest thing kenn
  puts on disk and must be inspectable and reclaimable from the CLI: `ls` reports
  each provisioned toolchain by language, resolved version, and size; `clean`
  gains `--toolchains` and `--toolchain <language>[@<version>]` so one toolchain
  can be dropped without razing the volume.
- **BREAKING** for every `runtime = "docker"` language: the published image no
  longer carries a toolchain, so the first index of a given version downloads it.
  Digest-pinned config keeps working; all six digests change.
- **No stopgap.** The currently published C# image stays as-is until this lands;
  we do not spend a CI republish on a toolchain-carrying image we are deleting.
  Consequence to accept explicitly: `runtime = "docker"` C# remains broken for
  repos with a `global.json` major pin until this change ships, so this is on the
  critical path rather than a background cleanup.

## Capabilities

### New Capabilities
- `toolchain-provisioning`: resolving a workspace's declared toolchain version,
  locating it in a shared cache, provisioning it when absent, and reporting
  progress and failure. Covers the concurrency, consent, and version-drift rules
  that apply to every language.
- `oci-image-build`: how indexer images are built and what they may contain —
  Docker from a committed Dockerfile, work done inside the build rather than on
  the host, payload but no toolchain, and verification by indexing a fixture.

### Modified Capabilities
- `docker-indexer-runtime`: images become thin and gain a toolchain cache volume
  mount; the per-language image no longer determines the toolchain version.
- `kenn-dotnet-runtime`: SDK selection becomes pin-driven with on-demand install,
  ordered before MSBuildLocator registration.

## Impact

- `docker/kenn-*/Dockerfile` — all six reworked: a builder stage for the
  entrypoint, then a runtime stage with no toolchain.
- `.github/workflows/images.yml` — updated for the reworked Dockerfiles; the
  existing buildx matrix stays.
- `crates/kenn-cli/src/init/detect.rs` — all six `IMG_*` digests refresh.
- New `kenn-toolchain` crate — the entrypoint binary baked into every image.
- `crates/kenn-indexer/src/workflow.rs` — toolchain volume alongside the existing
  per-language dependency cache volume.
- `kenn docker-cache` — a third volume kind, per-toolchain listing and reclaim.
- New network dependency at first index per toolchain version; offline use
  requires a pre-warmed cache volume.
