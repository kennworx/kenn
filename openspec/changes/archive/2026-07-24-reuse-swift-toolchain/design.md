## Context

The exact-vs-minimum gap lives at two mirrored decision points, one per side of the
container boundary:

```
HOST (kenn-indexer)                         CONTAINER (kenn-toolchain entrypoint)
provision_swift_from_image(swift:{pin})     run.rs: cache.is_provisioned("swift", pin)
  probes /t/{arch}/swift/{pin}  ── exact ──►   stats  /t/{arch}/swift/{pin}   ── exact ──
  miss → docker pull swift:{pin} (~5GB)        miss → Install::Preprovisioned → HARD ERROR
```

Both key on the literal pin string, so a provisioned `6.3` never satisfies a `6.0` pin.
The fix makes both sides select the **best provisioned toolchain `>=` pin** using one
shared rule.

## Decisions

**Design: both sides scan + a shared rule (not host-decides-via-env).**
The container already re-derives everything independently (`find_pin → resolve →
cache-check`), so swapping the one cache-check to a compatibility search is a minimal
diff. The env-var alternative (host picks, passes `KENN_SWIFT_TOOLCHAIN_VERSION`,
container obeys) needs new `-e` plumbing through `docker_launcher` and hits an ordering
problem — the Swift launcher command is built in `configure_runner` *before* the preflight
knows the reused version. Rejected.

**Agreement invariant.** The host preflight runs synchronously (`pipeline/api.rs`, `?`)
and is the only writer to the cache *before* any indexer container launches. So both
sides observe the same volume and, applying the identical `best_compatible`, select the
same toolchain. If nothing `>=` pin exists, the host provisions the pin → the container
then finds at least the pin. If something `>=` pin exists, the host skips → the container
reuses it. The only new bad outcome would be the container finding *nothing* usable — not
reachable from same-arch inputs once the enumerations match (below).

**Enumeration must match on both sides.** Host lists via a busybox `sh` glob
(`/t/{arch}/swift/*/` — dirs only, dotfiles skipped); container lists via `read_dir`
(`is_dir` + name not starting with `.`). Two fixes keep them byte-equivalent:
- Dot-prefix the host Swift staging dir → `.{version}.staging` (today `{version}.staging`
  is a real non-dot dir inside `swift/` that both enumerations would otherwise see; a
  strict parser drops it, but that is fragile — excluding it *structurally* is robust).
- `provisioned_versions` filters `is_dir()` and maps `NotFound → vec![]` (first-ever run).

Completeness rests on the cache's existing **atomic stage→rename** invariant: a listed
non-dot dir is complete; incompletes are the dot-prefixed staging. This is the same
guarantee the rest of the cache relies on, so no per-version `swiftc` probe is needed on
the reuse path.

**Selection rule: prefer-exact, else highest `>=` pin; cross-major allowed.**
- *Highest* (not lowest) converges the machine onto one newest Swift — disk is the whole
  motivation. The cost is attribution churn (an unchanged repo's `meta.json` Swift version
  can drift when another repo provisions a newer one), which triggers **no reindex**
  (`StalenessKey` ignores the toolchain version) and is honestly reported. `prefer-exact`
  damps most churn.
- *Cross-major* (a 6.3 toolchain satisfies a 5.9 pin) is where the second-pull savings
  live; a newer Swift builds an older tools-version package in the older language mode,
  and indexing is error-tolerant. `resolve_swift`'s existing doc note remains the escape
  hatch if an exact Swift is ever needed.

**No `semver` dependency.** None exists in the tree. `best_compatible` parses
`major.minor[.patch]` into a `(u32,u32,u32)` tuple and compares, mirroring the private
`SdkVersion` in `resolve/dotnet.rs` (minus its .NET band arithmetic).

## Risks

- Pre-existing (not introduced here, noted for completeness): the host Swift provision has
  no `flock`, so two concurrent kenn processes both resolving "provision the pin" race on
  `rm -rf {dest}; mv`. This change's scan-then-provision widens the window marginally.
- Arch mismatch between the host CLI's compile-time `Arch::host()` and an emulated
  cross-platform image is orthogonal and pre-existing (it breaks today's exact match too).
