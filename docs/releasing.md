# Releasing

Releases are built by [`dist`](https://github.com/axodotdev/cargo-dist). The
config lives in `dist-workspace.toml`; the workflow at
`.github/workflows/release.yml` is **generated from it**.

> Do not hand-edit the generated workflow. Change the config and run
> `dist generate` — CI checks that the committed workflow matches the config
> and fails if they have diverged.

## Cutting a release

```console
# bump [workspace.package] version in Cargo.toml, then:
git tag v0.1.0 && git push origin v0.1.0
```

That builds every target, attaches tarballs plus checksums to a GitHub release,
generates the shell installer, and publishes the Homebrew formula.

## Targets and runners

Every target builds on its **own architecture** — see
`[dist.github-custom-runners]`:

| target | runner |
|---|---|
| `aarch64-apple-darwin` | `macos-14` |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` |
| `x86_64-unknown-linux-gnu` | `ubuntu-22.04` |
| `x86_64-pc-windows-msvc` | dist default (verify with `dist plan`) |

**No Intel macOS** — not worth carrying a build for.

**Windows returned** (windows-support task 7). `live` is a pointer file now, so
`atomic_flip_live` no longer has a POSIX-only arm, and the whole workspace
compiles on Windows — the `ci-windows.yml` `cargo check` gate is green (after
fixing kenn-store's device-id arm and kenn-server's stale windows-sys FFI). Two
things `cargo check` does NOT prove: (1) the release build links + packages —
heavier than a check, so watch the first tagged Windows build; a broken target
still blocks publication for every platform (below); (2) that `kenn index`
actually runs — that is a manual smoke on a real unelevated Windows host
(task 7.3).

A listed target that fails does not fail alone: `host` and
`publish-homebrew-formula` run after the full matrix, so one broken target
blocks publication for every platform that built fine.

This is not incidental. `kenn-embed` vendors llama.cpp, so every build is also a
C++ build, and dist's default for aarch64 Linux is a cross toolchain. Emulation
is not an alternative: `rustc` SIGSEGVs under `qemu-x86_64` ("uncaught target
signal 11"), which is the same reason `docker/bake.hcl`'s publish uses native
runners.

Verify the mapping after any config change — the matrix is computed at run time,
so reading the workflow file will not tell you:

```console
dist plan --output-format=json | jq -r '.ci.github.artifacts_matrix.include[] | "\(.runner) \(.targets|join(","))"'
```

## The Homebrew tap

`brew install kennworx/tap/kenn` resolves to `github.com/kennworx/homebrew-tap`,
a **separate repository** — Homebrew requires the `homebrew-` name prefix. It
already exists and already carries formulae for other tools; dist writes
`Formula/kenn.rb` alongside them. Do not recreate or reinitialise it.

Pushing there is a cross-repo write, and the workflow's built-in `GITHUB_TOKEN`
is scoped to this repo only, so dist uses a `HOMEBREW_TAP_TOKEN` secret. That
already exists as a **kennworx org secret**, shared with the other repos that
publish to the tap; this repo has been added to its selected-repository list, so
no per-repo secret is needed.

If a release ever publishes but the formula does not update, check that list
first — an org secret with `visibility: selected` silently yields an empty value
in a repo that is not on it:

```console
gh api orgs/kennworx/actions/secrets/HOMEBREW_TAP_TOKEN/repositories \
  --jq '.repositories[].full_name'
```

## Sidecar indexers (kenn-ts, kenn-dotnet, kenn-swift)

None of the three is a cargo package — `kenn-ts` is `bun build --compile`,
`kenn-dotnet` is `dotnet publish --self-contained`, `kenn-swift` is
`swift build` — so `dist` neither sees nor builds them, and its
`extra-artifacts` does not help: those build in the GLOBAL job (verified with
`dist plan`), so they cannot emit per-platform binaries.

A separate workflow (`.github/workflows/sidecars.yml`) triggers on the same
`v*` tag, builds each sidecar in its own job, waits for the release `dist`
created — cross-workflow ordering is not expressible in Actions, so it polls
with a bounded wait rather than assuming order — uploads the per-platform
archives, then renders each formula from the uploaded `.sha256` files
(`.github/scripts/render-sidecar-formulas.sh`, which validates every checksum
before it writes anything) and pushes to the same tap with `HOMEBREW_TAP_TOKEN`.

**A partial release is expected, not a fault.** Each sidecar job is independent
(`fail-fast: false`, no cross-sidecar `needs`), so a missing Swift toolchain on
a runner costs the Swift formula, not the C# one. "The release succeeded" does
NOT imply every sidecar formula updated — the tap can hold a `kenn-swift` older
than `kenn`. That is acceptable only because the sidecars version in lockstep
with the CLI (one tag) and the JSONL wire is stable across a patch; a skew is a
version mismatch, not a protocol break. `kenn-swift` ships macOS arm64 only —
its `libIndexStore` dependency has no out-of-container Linux story.

## Container images

Indexer images are published separately by `.github/workflows/images.yml`, on
release or manual dispatch. They are versioned by digest rather than by this
tag — see [`docker/README.md`](../docker/README.md).
