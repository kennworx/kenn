## 1. Settle how non-cargo artifacts reach the release

- [x] 1.1 Determine whether `dist`'s `extra-artifacts` can build and attach
  PER-PLATFORM binaries, or only global ones. This decides the whole shape
  (D4) and is unverified. → verify: a throwaway config with one extra
  artifact, and `dist plan --output-format=json` showing whether it lands in
  the per-platform matrix or the global job.
  VERIFIED (dist 0.32.0): a throwaway `[[dist.extra-artifacts]]` shows up in
  `dist plan --output-format=json` with `target_triples: null`, grouped with
  the GLOBAL artifacts (`sha256.sum`, `source.tar.gz`) — never in the 8
  per-target entries. So extra-artifacts build ONCE in the global job and
  cannot emit per-platform binaries. → the separate workflow (1.2) it is.
- [~] 1.2 A separate workflow on the same `v*` tag, which polls for the release
  to exist with a bounded wait before uploading (extra-artifacts ruled out by
  1.1). → verify: it succeeds when started BEFORE dist's release job finishes —
  that is the race, so test that ordering specifically, not the lucky one.
  AUTHORED — `.github/workflows/sidecars.yml` (triggers on the same `v*` tag +
  workflow_dispatch) and `.github/scripts/publish-sidecar.sh`, which polls
  `gh release view <tag>` for ~20 min before uploading rather than assuming
  order. actionlint clean, scripts `bash -n` clean. The race check itself needs
  a real release run.

## 2. Build the sidecars in CI

- [~] 2.1 `kenn-ts` for macOS arm64 and Linux x64/arm64 via
  `bun build --compile --target=...`. Cross-compiles, so one runner can emit
  all three — the docker image already does this. → verify: each archive's
  binary reports the right architecture.
  AUTHORED in `.github/workflows/sidecars.yml` (kenn-ts job, one ubuntu runner,
  `--target=bun-{darwin-arm64,linux-arm64,linux-x64}`). Not CI-run yet — the
  per-archive arch check needs a real run.
- [~] 2.2 `kenn-dotnet` per RID (`osx-arm64`, `linux-x64`, `linux-arm64`),
  self-contained. → verify: runs on a machine with no .NET SDK installed.
  AUTHORED (kenn-dotnet job) — `dotnet publish -r <rid> --self-contained
  -p:PublishSingleFile=true` so the formula installs one binary. Not CI-run yet.
- [~] 2.3 `kenn-swift` for macOS arm64 only (D3). Resolve `libIndexStore`
  FIRST — it is not part of the OS, and a build made where Xcode is installed
  bakes an absolute `/Applications/Xcode.app` rpath. Decide between vendoring
  the library with an `install_name_tool` rewrite, `depends_on xcode:`, or
  building against the Command Line Tools path. → verify: `otool -L` and
  `otool -l | grep -A2 LC_RPATH` on the SHIPPED binary, then run it on a
  machine with only the Command Line Tools — the developer machine has Xcode
  and therefore cannot detect this failure.
  RESOLVED (design D3) — option 3: strip the Xcode rpaths, add
  `/Library/Developer/CommandLineTools/usr/lib` (where libIndexStore also
  ships, same ABI, and which every brew user has because Homebrew requires the
  CLT). Measured build/kenn-swift's `otool -L`/`LC_RPATH`, and demonstrated the
  `install_name_tool` rewrite carries the CLT rpath and still loads. Remaining
  (needs a CLT-only runner): load it with NO Xcode present — this machine has
  Xcode so cannot prove that half. Left `[~]` for that final CI-machine check.
- [x] 2.3a If vendoring `libIndexStore`, confirm its license permits
  redistribution before shipping it in a formula. → verify: the license is
  named in the change record, not assumed.
  N/A — 2.3 chose option 3 (CLT path), not vendoring, so nothing of the
  toolchain is redistributed and no license check is needed.
- [x] 2.4 Each job independent, `fail-fast: false`, none depending on another
  sidecar (D5). → verify: force one job to fail and confirm the others and
  the `kenn` release still publish.
  DONE structurally: sidecars.yml is three SEPARATE jobs with no cross-sidecar
  `needs`, so they are independent by construction (fail-fast is a matrix
  concept; separate jobs never block each other). The "force one to fail"
  run-check is a CI observation, but the independence is guaranteed statically.

## 3. Generate and publish the formulas

- [x] 3.1 A renderer taking version + artifact dir, reading each `.sha256`,
  emitting one formula. Validation BEFORE the template expands, in the parent
  shell — not inside a command substitution, where `exit 1` kills only the
  subshell (D6). → verify: missing checksum exits non-zero AND writes nothing;
  empty checksum likewise; restore and confirm it renders.
  DONE — `.github/scripts/render-sidecar-formulas.sh`. Validates every checksum
  in the parent shell (Phase 1) before writing any formula (Phase 2). Tested:
  all-valid renders three `.rb` (`ruby -c` → Syntax OK, class names KennTs /
  KennDotnet / KennSwift, kenn-swift macOS-only); a MISSING checksum and an
  EMPTY checksum each exit 1 with zero files written. (Fixed a portability bug
  found in testing: `class_for` used GNU-only `sed \U`, now awk.)
- [~] 3.2 Push to `kennworx/homebrew-tap` using `HOMEBREW_TAP_TOKEN`, adding
  formulas beside the existing ones. → verify: `Formula/kenn.rb` and the
  unrelated formulas already in that tap are untouched.
  AUTHORED in `.github/scripts/publish-sidecar.sh`: clones the tap, writes ONLY
  `Formula/<sidecar>.rb` (never touches `kenn.rb` or others), commits, and
  rebase-retries on a non-fast-forward (three jobs may push concurrently). Not
  run against the real tap yet.
- [~] 3.3 macOS: check whether Homebrew's install path invalidates the ad-hoc
  code signature the way `cp` does in `just install`. If it does, re-sign in
  the formula. → verify: install from the tap on a clean machine and run the
  binary — an unsigned binary is SIGKILLed by AMFI with no message, so
  "it installed" proves nothing.
  The swift job already re-signs (`codesign --force --sign -`) because
  `install_name_tool` invalidates the signature; the same AMFI trap applies to
  Homebrew's copy-into-Cellar. Whether Homebrew itself needs a `codesign` step
  can only be settled by a clean-machine `brew install` (6.1) — left `[~]`.

## 4. Make `kenn init` name the formulas

- [x] 4.1 Update the install hints in `crates/kenn-cli/src/init/detect.rs` for
  the three kenn-authored indexers to name their Homebrew formula. → verify:
  `kenn init` in a C# workspace with no `kenn-dotnet` prints a hint naming the
  formula, and the existing hint test still passes.
  Hints updated to `brew install kennworx/tap/kenn-{ts,dotnet,swift}` (from
  source: just build-indexer-…), and a test added
  (`kenn_authored_hints_name_the_homebrew_formula`) asserting the three name
  their formula and the third-party three do NOT. VERIFIED: `cargo test -p kenn
  hint` → 4 passed, 0 failed, including the new test and the existing
  `a_failing_probe_degrades_with_command_and_hint`.
- [x] 4.2 Keep third-party hints as they are — `rust-analyzer`, `scip-go`,
  `scip-python` are not ours to package. → verify: those hints are unchanged.
  Verified: rust `rustup component add`, go `go install …scip-go`, python
  `npm install …scip-python` — untouched; the new test also asserts none names
  a `kennworx/tap` formula.

## 5. Documentation

- [x] 5.1 README: per-indexer install commands alongside the existing
  third-party section, making clear which come from kenn and which do not.
  Added "Installing kenn's indexers" (brew install kennworx/tap/kenn-{ts,dotnet,
  swift}, swift macOS-only) before the third-party section.
- [x] 5.2 `docs/releasing.md`: how sidecar release differs from the dist-owned
  CLI release, and that a partial release is expected rather than a fault.
  Added a "Sidecar indexers" section: not cargo packages, separate workflow on
  the same tag polling for the release, formulas rendered from published
  checksums, and independent jobs → a partial release is expected.

## 6. Verify the way users actually consume it

- [ ] 6.1 `brew install` each of the four formulas on a clean machine and run
  each binary. A tap has no external review — nothing catches a malformed
  formula except installing it, so a successful publish is NOT verification.
- [ ] 6.2 End-to-end: install `kenn` + `kenn-dotnet` only, then
  `kenn init` && `kenn index` a real C# repository cloned from GitHub, and
  confirm a symbol count rather than exit 0. → verify: C# is enabled, not
  degraded to the text fallback.
