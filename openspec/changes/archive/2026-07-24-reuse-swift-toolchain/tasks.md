## 1. Shared selector (`kenn-toolchain`)

- [x] 1.1 Add a `select` module (declared in `lib.rs`, code in `select.rs`) with
  `pub fn best_compatible(pin: &str, available: &[String]) -> Option<String>` and an
  internal `parse "major.minor[.patch]" -> (u32,u32,u32)` (missing components = 0).
  Rule: exact string match preferred; else highest parseable `>= pin`; else `None`;
  unparseable pin with no exact match → `None`. → verify: unit tests —
  `("6.0", ["6.0","6.3"]) == "6.0"` (exact preferred); `("6.1", ["6.0","6.3"]) == "6.3"`;
  cross-major `("5.9", ["6.3"]) == "6.3"`; `("6.5", ["6.3"]) == None`; component mismatch
  `("6.0.0", ["6.0"])` and `("6.0", ["6.0.0"])` resolve; `".6.0.staging"` / unparseable
  entries are dropped.

## 2. Cache version listing (`kenn-toolchain`)

- [x] 2.1 Add `ToolchainCache::provisioned_versions(&self, language: &str) -> Vec<String>`
  (`crates/kenn-toolchain/src/cache.rs`): read `self.root.join(language)`, keep entries
  that `is_dir()` and whose name does not start with `.`; map `NotFound` → `vec![]`. →
  verify: returns the version dir names, excludes a `.lock`/`.staging` entry, and returns
  empty for a missing language dir.

## 3. Container reuse gate (`kenn-toolchain`)

- [x] 3.1 In `run.rs` at the provisioning decision (currently
  `cache.is_provisioned(language.key(), &resolved.version)`): for `Language::Swift`, use
  `best_compatible(&resolved.version, &cache.provisioned_versions("swift"))` → `Some(best)`
  yields `Outcome::AlreadyPresent { version: best, path: cache.path("swift", &best) }`;
  `None` falls through to the existing `Install::Preprovisioned` error. Non-Swift keeps
  the exact `is_provisioned` check. → verify (mutation-checked): with a cache holding only
  `6.3`, a `6.0`-pinned Swift workspace resolves to `AlreadyPresent { version: "6.3" }`;
  reverting the Swift branch to the exact check makes it hard-error.

## 4. Host reuse gate (`kenn-indexer`)

- [x] 4.1 Dot-prefix the Swift staging dir in `run_swift_provision`
  (`crates/kenn-indexer/src/docker.rs`): `.{dest_version}.staging` under
  `/t/{arch}/swift/`, so it is excluded by both the busybox glob and the container's
  non-dot filter. → verify: the provision script targets a dotfile staging path; a listed
  version set never contains a `*.staging` name.
- [x] 4.2 Replace the exact `swift_toolchain_present(dest)` idempotency probe in
  `provision_swift_from_image` with a compatibility check: enumerate
  `/t/{arch}/swift/*/` via one busybox run (dir names, dotfiles skipped) →
  `best_compatible(dest_version, &versions).is_some()` → skip the pull; else provision
  `swift:{dest_version}` unchanged. → verify (mutation-checked): with `6.3` provisioned
  and `6.0` absent, provisioning for a `6.0` pin is skipped (no `docker run swift:6.0`);
  reverting to the exact probe re-triggers the pull.

## 5. Verification

- [x] 5.1 Live end-to-end: provision swift `6.3`, remove swift `6.0` from the cache
  volume, then `kenn index -w <swift-repo pinning 6.0> --force`. → verify: no `swift:6.0`
  pull, the index completes with the repo's symbols, and the `toolchain` frame / `meta.json`
  report swift `6.3`.
- [x] 5.2 Gates: `cargo clippy --workspace --all-targets`, `just crap-ci`, `cargo fmt --all`.
