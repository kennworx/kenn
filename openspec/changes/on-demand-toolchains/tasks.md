## 1. `kenn-toolchain` — the in-container provisioning entrypoint

- [x] 1.1 Define the cache layout (`<root>/<lang>/<resolved-version>/`) and the
      atomic-install contract: unpack to a staging dir, exclusive lock on the
      destination key, rename into place. Mutation-check: interrupt an install
      mid-unpack, confirm the next run does not treat the partial tree as
      complete.
- [x] 1.2 Stand up the `kenn-toolchain` crate: library plus the binary that every
      image runs as ENTRYPOINT, execing the real indexer behind it.
- [x] 1.3 Fetch + verify + unpack, in-process (HTTP, gzipped tar). No `sh`, no
      `curl`, no package manager — that is what keeps the images scratch.
      Checksum-verify against the vendor's published hash BEFORE unpacking;
      a missing or mismatched hash fails the install.
- [x] 1.4 Mount the toolchain cache volume in the docker runtime alongside the
      existing per-language dependency cache volume, and export the toolchain
      root into the container.
- [x] 1.5 Emit progress before/after a fetch. Mutation-check with a stubbed slow
      install: the "provisioning" signal must reach the consumer *before* the
      fetch begins, not batched after it.
- [x] 1.5a Per-language pin readers in the entrypoint, and the vendor release
      metadata each one needs for URL + published checksum. Do NOT construct
      artifact URLs by pattern — they come from the vendor's release index.
- [x] 1.6 Extend `kenn docker-cache`: list the toolchain volume with each
      provisioned toolchain's language, resolved version, and size; add
      `clean --toolchains` and `clean --toolchain <lang>[@<version>]`. Confirm
      `--orphans` leaves it intact (it is bound to no directory).
- [ ] 1.7 Record the resolved toolchain version with the index and report it in
      the run summary. Do NOT add a second staleness input — pin files are
      tracked, so the existing staleness key already covers edits.
- [x] 1.8 Make an unresolvable or uninstallable pin a **fatal, named** failure —
      quote the pin and its source file; never fall back to a present toolchain.
      Mutation-check: break resolution, confirm non-zero exit and the pin in the
      message.

## 2. Image builds (Docker, as much as possible inside the build)

- [x] 2.1 Keep `docker build` and a Dockerfile per language. No bespoke OCI
      assembler: it would be ours to maintain and one bug would break publishing
      for every language at once.
- [x] 2.2 Compile `kenn-toolchain` in a Dockerfile builder stage, per target
      platform — NOT cross-compiled on the host and copied in. The host needs
      nothing but Docker, and no aarch64-musl toolchain has to exist on it.
- [x] 2.3 Multi-arch as today (buildx), since the entrypoint and each indexer
      binary are built inside the per-platform build.
- [ ] 2.4 Verify a published image by pulling it and indexing a fixture — not by
      running `--version`, which passes on an image missing its CA bundle.

## 3. Per-language pin readers and installers

- [x] 3.1 C#: `global.json` → concrete SDK version (honoring `rollForward`) →
      .NET SDK tarball. Pilot language.
- [x] 3.2 Swift — RESOLVED by provisioning from the official image (a digest-pinned image IS a verified artifact). Original blocker: swift.org publishes
      neither a download URL nor any checksum for Linux toolchains: release
      metadata carries only `{name, platform, archs, docker}`, the `.sha256` and
      `SHA256SUMS` endpoints 404, and the sole integrity artifact is a detached
      PGP `.sig`. The `checksum` fields in that JSON belong to the static-sdk /
      wasm-sdk pseudo-platforms, NOT the toolchain tarball. So Swift cannot meet
      the verification requirement as the other five do. Options: (a) verify the
      PGP signature (swift.org's own sanctioned method, needs a PGP
      implementation and their signing keys), (b) ship a kenn-maintained table of
      self-computed hashes — breaks for any version we have not seen, (c) leave
      Swift on `runtime = "local"`. Do NOT silently downgrade to TLS-only.
- [x] 3.3 Rust: `rust-toolchain.toml` → rustup, with `RUSTUP_HOME`/`CARGO_HOME`
      in the cache and `rust-src` present for rust-analyzer.
- [x] 3.4 Go: set `GOTOOLCHAIN=<exact version from go.mod>` and mount the module
      cache; do not write a resolver. NOT `auto` — measured, `auto` only ever
      switches UPWARD (a repo pinning go1.24.5 under a local go1.26.5 stays on
      1.26.5), so `auto` guarantees "at least the pin", never "exactly the pin".
      Go then self-provisions into GOMODCACHE with sumdb verification.
- [x] 3.5 Python: `.python-version` / `requires-python` → `uv python install`,
      plus **node** as a toolchain for scip-python itself.

## 4. Thin images

- [x] 4.1 Rework each Dockerfile to the shared shape: a builder stage compiling
      `kenn-toolchain`, then a small runtime base carrying that entrypoint, the
      tool binary, `git` + CA certificates, and NO language toolchain.
      Base chosen per language by what the vendor ships: alpine for csharp,
      rust, go and typescript (first-class musl artifacts); glibc for python
      (Node publishes no official musl build) and swift (no musl toolchain
      exists at all). Do not force alpine where it costs more than it saves.
      csharp DONE and verified; five remain.
- [ ] 4.2 Determine each indexer's spawned-executable set by OBSERVING a real
      index run (strace/dtrace or equivalent), not by reading code. Known so far:
      kenn-ts spawns `git worktree list`; scip-python spawns `pip list` and reads
      the project version from `git`. Ship what each actually spawns.
- [ ] 4.3 Build and verify each of the six images by indexing a real fixture:
      csharp (pinned non-latest major), swift (the existing Linux/Alamofire run),
      typescript, rust, go, python.
- [x] 4.4 Set `kenn-toolchain` as the ENTRYPOINT of every image, execing the real
      indexer behind it, and confirm each indexer's argv still reaches it intact.

## 5. Publish and cut over

- [x] 5.1 Update `.github/workflows/images.yml` for the reworked Dockerfiles;
      the existing buildx matrix stays.
- [ ] 5.2 Republish and re-pin all six `IMG_*` digests in
      `crates/kenn-cli/src/init/detect.rs`.
- [ ] 5.3 Document the offline story: what a pre-warmed cache volume requires and
      how to populate one.
- [ ] 5.4 Record measured before/after image size, first-index timing, and
      warm-cache timing per language, so the trade is verifiable rather than
      asserted.
