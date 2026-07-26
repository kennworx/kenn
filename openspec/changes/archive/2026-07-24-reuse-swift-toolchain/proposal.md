## Why

`swift-tools-version` in a `Package.swift` is a **minimum**, not an exact version —
a swift 6.3 toolchain builds a package that declares `swift-tools-version:6.0`. Every
other language pins an exact toolchain (`global.json`, `go.mod`, …); Swift alone
declares a floor. But kenn treats the Swift pin as exact:

- the host preflight provisions `swift:{pin}` by pulling the full official image
  (`crates/kenn-indexer/src/docker.rs`, `provision_swift_from_image`), and
- the in-container entrypoint looks for a cache dir named **exactly** `{pin}`
  (`crates/kenn-toolchain/src/run.rs`, `cache.is_provisioned(key, resolved.version)`),
  erroring hard otherwise.

So a repo pinning `6.0` pulls `swift:6.0` (~5 GB) and copies ~2 GB into the cache
**even when swift 6.3 is already provisioned** and would satisfy the minimum. This is
the common case — most repos pin older-than-baked — and it is the last remaining cost
after the recent provisioning idempotency+visibility fix.

`resolve_swift` already flags this: *"`swift-tools-version` is a MINIMUM … worth
revisiting if a workspace ever needs an exact Swift."* This change revisits it.

## What Changes

- Add a shared selector `best_compatible(pin, &[version]) -> Option<version>` in
  `kenn-toolchain` (new `select` module): prefer an exact string match of the pin,
  else the **highest** provisioned version `>=` pin (cross-major allowed, since a newer
  toolchain builds an older tools-version in the older language mode), else `None`.
  Internal `major.minor[.patch]` tuple compare, mirroring `resolve/dotnet.rs`'s
  `SdkVersion` (no `semver` crate exists in the tree).
- Add `ToolchainCache::provisioned_versions(language) -> Vec<String>`
  (`crates/kenn-toolchain/src/cache.rs`): the provisioned version dirs under the
  language cache (`is_dir` + name not starting with `.`; a missing dir → empty).
- **Container gate** (`run.rs`): for Swift only, select `best_compatible` over the
  provisioned versions instead of an exact-name lookup; report and PATH the chosen
  version. Other languages keep the exact check.
- **Host gate** (`docker.rs`): skip the pull when `best_compatible` finds a compatible
  provisioned toolchain; otherwise provision `swift:{pin}` exactly as today.
- **Robustness:** dot-prefix the host Swift staging dir (`.{version}.staging`) so an
  in-flight/crashed provision is structurally excluded from both enumerations, keeping
  the host busybox glob and the container `read_dir` in agreement.

The host preflight runs to completion before the indexer container starts and is the
only writer, so both sides scan the same cache and — using the identical rule — select
the same toolchain.

## Capabilities

### Modified Capabilities

- `toolchain-provisioning`: for Swift, the declared `swift-tools-version` is treated as
  a **minimum** — a provisioned toolchain whose version is `>=` the pin satisfies it and
  is reused, rather than provisioning the exact pinned version. Every other language's
  exact-pin behavior is unchanged.

## Impact

- **No new dependencies, no schema change.** ~120–160 lines across `kenn-toolchain`
  (`select`, `cache`, `run`) and `kenn-indexer` (`docker`).
- **Attribution:** `meta.json` / the `toolchain` wire frame report the *reused* version
  (e.g. `6.3` for a `6.0` pin) — honest, since that toolchain built the index.
- **No reindex churn:** `StalenessKey` keys on git + language-config signature, never the
  toolchain version, so a reused (or later drifted) version neither skips nor forces a
  reindex.
- **Migration:** none. Existing caches keep working; the first index after the change
  simply reuses a compatible toolchain instead of pulling. `runtime = "local"` is
  unaffected.
