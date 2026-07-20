## 1. Settle how non-cargo artifacts reach the release

- [ ] 1.1 Determine whether `dist`'s `extra-artifacts` can build and attach
  PER-PLATFORM binaries, or only global ones. This decides the whole shape
  (D4) and is unverified. → verify: a throwaway config with one extra
  artifact, and `dist plan --output-format=json` showing whether it lands in
  the per-platform matrix or the global job.
- [ ] 1.2 If it cannot: a separate workflow on the same `v*` tag, which polls
  for the release to exist with a bounded wait before uploading. → verify:
  it succeeds when started BEFORE dist's release job finishes — that is the
  race, so test that ordering specifically, not the lucky one.

## 2. Build the sidecars in CI

- [ ] 2.1 `kenn-ts` for macOS arm64 and Linux x64/arm64 via
  `bun build --compile --target=...`. Cross-compiles, so one runner can emit
  all three — the docker image already does this. → verify: each archive's
  binary reports the right architecture.
- [ ] 2.2 `kenn-dotnet` per RID (`osx-arm64`, `linux-x64`, `linux-arm64`),
  self-contained. → verify: runs on a machine with no .NET SDK installed.
- [ ] 2.3 `kenn-swift` for macOS arm64 only (D3). → verify: `otool -L` shows
  it links the OS-provided Swift runtime, and it runs on a clean macOS.
- [ ] 2.4 Each job independent, `fail-fast: false`, none depending on another
  sidecar (D5). → verify: force one job to fail and confirm the others and
  the `kenn` release still publish.

## 3. Generate and publish the formulas

- [ ] 3.1 A renderer taking version + artifact dir, reading each `.sha256`,
  emitting one formula. Validation BEFORE the template expands, in the parent
  shell — not inside a command substitution, where `exit 1` kills only the
  subshell (D6). → verify: missing checksum exits non-zero AND writes nothing;
  empty checksum likewise; restore and confirm it renders.
- [ ] 3.2 Push to `kennworx/homebrew-tap` using `HOMEBREW_TAP_TOKEN`, adding
  formulas beside the existing ones. → verify: `Formula/kenn.rb` and the
  unrelated formulas already in that tap are untouched.
- [ ] 3.3 macOS: check whether Homebrew's install path invalidates the ad-hoc
  code signature the way `cp` does in `just install`. If it does, re-sign in
  the formula. → verify: install from the tap on a clean machine and run the
  binary — an unsigned binary is SIGKILLed by AMFI with no message, so
  "it installed" proves nothing.

## 4. Make `kenn init` name the formulas

- [ ] 4.1 Update the install hints in `crates/kenn-cli/src/init/detect.rs` for
  the three kenn-authored indexers to name their Homebrew formula. → verify:
  `kenn init` in a C# workspace with no `kenn-dotnet` prints a hint naming the
  formula, and the existing hint test still passes.
- [ ] 4.2 Keep third-party hints as they are — `rust-analyzer`, `scip-go`,
  `scip-python` are not ours to package. → verify: those hints are unchanged.

## 5. Documentation

- [ ] 5.1 README: per-indexer install commands alongside the existing
  third-party section, making clear which come from kenn and which do not.
- [ ] 5.2 `docs/releasing.md`: how sidecar release differs from the dist-owned
  CLI release, and that a partial release is expected rather than a fault.

## 6. Verify the way users actually consume it

- [ ] 6.1 `brew install` each of the four formulas on a clean machine and run
  each binary. A tap has no external review — nothing catches a malformed
  formula except installing it, so a successful publish is NOT verification.
- [ ] 6.2 End-to-end: install `kenn` + `kenn-dotnet` only, then
  `kenn init` && `kenn index` a real C# repository cloned from GitHub, and
  confirm a symbol count rather than exit 0. → verify: C# is enabled, not
  degraded to the text fallback.
