## 1. Foundation — report out-of-root documents (independent bug)

- [x] 1.1 In `crates/kenn-indexer/src/pipeline/ingest.rs:301-307`, split the
      `OutsideRoot` arm out of the silent `Excluded`/`UnknownLanguage` skip: count
      it and record a `RunReport` entry naming `project_root` vs kenn's root.
      Keep `Excluded` silent (exclusion is intentional).
      Verify: unit test feeds a SCIP doc whose `project_root` is `/work` while the
      workspace root is a temp dir; assert the run reports ≥1 dropped-outside-root
      document and does NOT report clean success. **Mutation-check (§9)**: revert
      the split, confirm the test goes red.
- [x] 1.2 Summary + exit contract. The gate is run-level: `out_of_root_drops > 0`
      **and** total in-root docs `== 0` → non-zero exit (scripted `init && index`
      stops). Drops with in-root docs surviving → warning + partial snapshot (exit
      0), mirroring the existing `rust indexed 0 files` path. An honestly-empty run
      (no drops — empty repo, all-`Excluded`, or a producer that emitted nothing)
      stays exit 0.
      Verify: `cli_smoke` covers three cases — all-dropped → non-zero; partial drop
      → warn + exit 0; empty repo → exit 0, no failure. Mutation-check the
      all-dropped exit (revert → all-dropped wrongly exits 0) AND that an empty repo
      is not caught by the gate (guards the `drops > 0` half).

## 2. Config surface — `runtime` + `image`

- [x] 2.1 Add `runtime: Runtime` (enum `Local` default | `Docker`) and
      `image: Option<String>` to each of the six code-driver structs
      (`crates/kenn-config/src/language/{csharp,rust,typescript,python,go,swift}.rs`),
      their `#[serde(default …)]`, and their hand-written `Default` impls
      (`runtime = Local`, `image = None`).
      Verify: `Config::from_toml` round-trips a `[language.rust] runtime="docker"
      image="…"` block; `deny_unknown_fields` still holds; a config without the
      fields loads with `runtime = Local`.
- [x] 2.2 `Config::validate` (`config.rs:96-108`): reject `runtime = "docker"`
      with no `image`, and (optionally) a non-empty `image` under `runtime =
      "local"` as a likely mistake.
      Verify: `validate` unit tests for both rejection cases. Mutation-check the
      docker-without-image case.
- [x] 2.3 Confirm `indexing_signature` covers the new fields (it hashes all of
      `self.language`).
      Verify: extend `indexing_signature_tracks_language_not_staleness` — flipping
      `runtime` changes the signature.

## 3. Docker runtime (POSIX, same-path mount)

- [x] 3.1 A `MountStrategy` enum (`SamePath { root }` now; `Translate` is the
      Windows follow-on seam, not implemented here) and a launcher-rewrite helper
      that wraps a configured `command` into the docker invocation: `--rm`,
      `--user <uid>:<gid>` + `-e HOME=/tmp` (so files land user-owned, not root),
      `-v <root>:<root> -w <root>`, `<image>`, then the original `command`.
      Verify: unit test asserts the synthesized argv for a rust config — includes
      `--user`, the same-path `-v`/`-w`, and ends with the original launcher; pure
      string/vec assertion, no daemon.
- [x] 3.2 In `configure_runner` (`workflow.rs:220-278`), when a language's
      `runtime = Docker`, rewrite its `command` via 3.1 before cloning it into the
      driver struct. Drivers are untouched (their absolute-path args are valid
      under the same-path mount).
      Verify: unit test on `configure_runner` output — the rust driver's `command`
      begins with `docker run …` and ends with the original launcher.
- [x] 3.3 Docker availability preflight: a runnable `docker` (single `docker
      info`-class probe) is required when any language is `runtime = Docker`;
      absence is a reported, actionable error (never a silent local fallback).
      Verify: with `docker` scrubbed from `PATH`, a docker-runtime config fails
      with the actionable message; test asserts the message, not just non-zero.
- [x] 3.4 Caches, split by shareability (`[docker]` config, commented in the
      starter template):
      - **Dependency sources** (`CARGO_HOME`/`GOMODCACHE`/`NUGET_PACKAGES`/
        `PIP_CACHE_DIR`/`npm_config_cache`) → a **shared named volume**
        (`cache_volume`, default `kenn-docker-cache`), cross-repo. A named volume,
        not a bind mount, because a bind-mounted hot cache is severely slow across
        the host↔VM boundary on macOS/Windows.
      - **Build artifacts** (`CARGO_TARGET_DIR`, `GOCACHE`) → **ephemeral** by
        default (in-container `/tmp/kenn-build`, dropped on `--rm`); with
        `persist_build_cache = true`, a **per-workspace** named volume
        (`kenn-build-<hash>`). Never shared — cargo locks `target/`.
      Verify: argv asserts the shared source volume + ephemeral build env; a second
      test asserts the per-workspace build volume under `persist_build_cache`
      (mutation-checked). ✓
- [x] 3.5 End-to-end. `just docker-index-smoke` (committed, docker-gated): builds
      the throwaway `docker/rust-analyzer` image, indexes a minimal rust crate with
      `runtime = "docker"`, and asserts a non-empty `rust.scip` owned by the
      invoking user (not root). The fixture lives under the repo's `./tmp` (below
      `/Users`, which Docker Desktop shares — the mac default `$TMPDIR`
      `/var/folders` is NOT shared, so a mount there is empty) and is nested in
      kenn's cargo workspace, so it also exercises the nesting fix (only the crate
      reaches the container). Enabling code: `ensure_cache_volume` (create + chown
      the root-owned named volumes so a `--user` container can write them) and the
      absolute-workspace fix (a relative `-w` made the scip `--output` relative,
      unresolvable by `docker run`; now canonicalized before `Layout::resolve`).
      Also proven manually against `fd` (458 rust symbols; warm cache 50s → 15s).
      A `--network=none` strict-offline assertion and a digest-pinned published
      image are phase-4 refinements.
- [x] 3.6 `kenn docker-cache` — list/remove kenn's cache volumes: per-worktree
      `kenn-build-<hash>` and per-repo `kenn-deps-<hash>` (both created + labelled by
      task 3.7). New `Command::DockerCache` mirrors `Command::Gc`. The whole command is
      **config-free** — it operates on labelled docker volumes and the current worktree
      root, never reading `kenn.toml`. Enumerate by the `kenn.managed` label
      (`docker volume ls --filter label=kenn.managed`) so ALL kenn volumes are found
      (incl. the shared deps volume, which has no `kenn.workspace`); kind comes from the
      `kenn-build-`/`kenn-deps-` name prefix.
      Modes — `ls` lists volumes with kind + bound `kenn.workspace` path (or `shared`) +
      on-disk existence + in-use (`--json`); `clean` (no flag) removes the current
      worktree's build volume; `--orphans` removes every `kenn.managed` volume that HAS a
      `kenn.workspace` label whose dir no longer exists (worktree drop → its build; repo
      delete → its build + deps; crash self-heal; shared volume never orphaned); `--all`
      removes every `kenn.managed` volume incl. the shared one; `--workspace <path>`
      targets an existing worktree's build volume (deps not directly targetable — use
      `--orphans`/`docker volume rm`). Build-scope flags **mutually exclusive** (clap
      `ArgGroup`).
      Exit-status contract: absent → `NothingToRemove`/exit 0; in-use → `SkippedInUse`/
      exit 0; genuine docker failure → exit non-zero (even mid-sweep); `docker` not
      runnable → `clean` exits 0 (teardown-safe), `ls` errors.
      Verify: hermetic unit tests for the orphan predicate (labelled dir exists → kept;
      missing → reaped; unlabelled shared volume never orphaned) and the outcome→exit
      mapping; the daemon-touching path (label round-trip via `volume inspect`,
      `--orphans` reaping a volume whose labelled dir was removed) rides the
      `docker-index-smoke` gate. **Mutation-check (§9)**: invert the orphan predicate
      (reap when the dir exists) and confirm the kept-live-volume test fails.
- [x] 3.7 Per-repo dependency cache + volume labels (creation side). Today §3.4 mounts
      ONE shared cross-repo deps volume (`cache_volume` = `kenn-docker-cache`). Change
      the DEFAULT to a **per-repository** deps volume `kenn-deps-<hash(main-worktree)>`,
      resolving the repo's main worktree from the current workspace via git (the main
      worktree's workdir; the workspace root when not a worktree) so all of a repo's
      linked worktrees share one deps cache. Config `cache_volume` becomes
      `Option<String>`: unset → per-repo; set → that single shared cross-repo volume
      (bound to nothing). **Two-label scheme** at creation (`ensure_cache_volume`): every
      kenn-created volume gets `kenn.managed=true` (enumeration key — the shared
      configured volume included); each bound volume ALSO gets `kenn.workspace=<bound-dir>`
      (build bound to the worktree, deps to the main worktree); the shared configured
      volume gets `kenn.managed` only. Move `build_volume_name` → `docker.rs`
      (canonicalizes internally) and reuse it for both kinds keyed on their bound dir.
      Verify: creation test asserts a per-repo `kenn-deps-<hash>` bound+labelled
      (`kenn.managed` + `kenn.workspace`) to the main worktree by default, and the
      configured shared volume `kenn.managed`-only; a linked worktree reuses the repo's
      deps volume. **Mutation-check (§9)**: bind the deps volume to the worktree instead
      of the main worktree and confirm the shared-across-worktrees test fails.

> Windows path translation (`MountStrategy::Translate`, `/work` mount, SCIP
> `project_root` reconciliation) is a **separate follow-on change** — see
> `proposal.md` Deferred. The `MountStrategy` seam built in 3.1 is where it lands.

## 4. Published images + CI (kenn-owned)

- [~] 4.1 A Dockerfile per indexer under `docker/`. **All six done + verified**
      (built locally, host arch; the tool answers inside the image):
      external tools `docker/{kenn-rust,kenn-go,kenn-python}` (rust-analyzer
      1.97, scip-go 0.2.7, scip-python 0.6.6), and the sidecars `docker/kenn-typescript`
      (bun-compiled, slim base + git) and `docker/kenn-csharp` (framework-dependent
      publish onto the .NET SDK base — the SDK stays at runtime for MSBuildLocator
      + `dotnet restore`). Sidecars build from `indexers/<name>` via
      `docker/<name>/Dockerfile` (bake kenn's own binary, not an upstream tool);
      each context has a `.dockerignore` scrubbing host build output. `just
      build-image <name>` builds any one locally.
      **`kenn-swift` now containerized** (was blocked by a `Foundation.Process`
      deadlock in `swift:6.x` Linux containers, which bit twice — both fixed):
      (1) RUNTIME — kenn-swift spawns `swift build` via a `posix_spawn`
      `ProcessRunner`, never `Foundation.Process` (commit `ac826f0`); (2) BUILD-TIME
      — `swift-index-store`'s `Package.swift` spawned `Process` during manifest
      evaluation, so `docker/kenn-swift/Dockerfile` overwrites that manifest with one
      reading `KENN_TOOLCHAIN_LIB` from the env (no subprocess) and points kenn-swift's
      dependency at the local patched copy. The image (swift:6.3, toolchain kept for
      runtime `swift build`) builds clean in ~220s and, run against a Swift package,
      emits a valid index (struct + init + method, correct kinds/sigs) with no hang.
      (Bonus fix already landed: scip-go moved `sourcegraph/scip-go` →
      `scip-code/scip-go`; kenn's `init` install hint in `detect.rs` was corrected.)
- [x] 4.2 `.github/workflows/images.yml`: multi-arch (amd64+arm64) build + push to
      `ghcr.io/${{ github.repository_owner }}/<name>` (kennworx) on release +
      manual dispatch, via the built-in `GITHUB_TOKEN` (`packages: write`).
      Matrix covers all six images (three external tools + kenn-ts + kenn-dotnet +
      kenn-swift), carrying a per-image `context`/`file` so the sidecars build from
      their source dir. **Proven end-to-end**: repo pushed (`kennworx/kenn`,
      private), `workflow_dispatch` on `main` built + pushed all six multi-arch —
      the green run validates the workflow better than `actionlint` would.
      **Public visibility DEFERRED by decision**: the repo + the six packages stay
      `private` for now and will be made public together later (GitHub has no REST
      endpoint for container-package visibility — web-UI only). While private,
      `kenn init --docker` authors a correct config and the pull just needs a
      one-time `docker login ghcr.io` (verified: all six `check all` end-to-end
      passes ran against the private images after a login). Flipping to public
      later needs **no code change** — the digest pins don't move.
      **Named by language** (`kenn-<lang>`, matching the `[language.X]` keys):
      `ghcr.io/kennworx/{kenn-rust,kenn-go,kenn-python,kenn-typescript,kenn-csharp,kenn-swift}`.
      Renaming was done in two steps: first a fast `crane copy` (blob-mount,
      digests preserved) to prove the naming, then a full **CI rebuild** (run
      #29647297070) so every image is built by GitHub Actions with provenance and
      **auto-linked to the repo** — the crane copies aren't. `images.yml` + the
      `docker/kenn-<lang>` build-context dirs + `justfile` all carry the new names;
      the Actions were bumped to node24 majors (commit `71a8698`). The five
      old-named packages (`rust-analyzer`/`scip-go`/`scip-python`/`kenn-ts`/
      `kenn-dotnet`) are deleted; the six `kenn-<lang>` packages remain.
- [x] 4.3 Wire default images (by digest) into the language defaults so `kenn init
      --docker` (phase 5) resolves a real published image. Done: six digest-pinned
      `IMG_*` constants in `crates/kenn-cli/src/init/detect.rs`, each a
      `ghcr.io/kennworx/<name>@sha256:<manifest-list-digest>` (verified multi-arch
      via `docker buildx imagetools inspect`), threaded onto `LanguageSpec.default_image`.
      Verify: the default-image constants are digest-pinned refs to
      `ghcr.io/kennworx/*` (unit test asserts the `ghcr.io/kennworx/…@sha256:` shape).

## 5. `kenn init --docker` (depends on phase 4 defaults)

- [x] 5.1 Add `--docker` to `kenn init`. When set and `docker` is runnable, a
      language whose local toolchain probe fails and whose default image (4.3) is
      known is authored as `runtime = "docker"` + that image instead of degrading
      to text; report it as `containerized`. Done: new `Availability::Containerized`
      variant produced by `classify_with(.., containerize)` when the probe fails and
      `default_image` is `Some`; `author::enable_docker_language` writes
      `enabled/runtime/image`; `report` renders + counts it.
      Verify: hermetic `detect`/`author` tests (stubbed probe + `containerize`
      flag) assert `runtime = "docker"`; the no-`--docker` contrast degrades to
      text. **Mutation-checked (§9)**: `filter(|_| false)` on the container branch
      turned the result back to `Degraded` (test went red); `runtime = "local"`
      failed the author test; both restored. Also proven end-to-end against the
      real binary on a Go fixture (scip-go absent) → authored the pinned digest.
- [x] 5.2 `--docker` with `docker` absent is a reported error and detection falls
      back to the existing degrade report. Done: pure `containerize_decision(opt_in,
      daemon_up)` returns `(false, Some(msg))` when opted-in but the daemon is down;
      `run` prints `msg` and proceeds with `containerize = false`.
      Verify: `containerize_decision_covers_the_three_cases` asserts the message +
      the no-containerize fallback across all three input combinations.

## 6. Content-addressed SCIP-output cache — DROPPED (see design.md D5)

Dropped after design review: caching the SCIP *output* forces kenn to own
invalidation (which files feed each language's indexer), which is not reliably
determinable — under-hashing yields a false hit → a silently wrong index, worse
than no cache. The reliable caching is the deps + build-intermediate volumes
(§3.4 / D8), where the toolchain owns invalidation. 6.1's key helper was
implemented then reverted.

- [~] ~~6.1 Determine the image digest + compute `cache_key`.~~ Dropped (reverted).
- [~] ~~6.2 Shared SCIP-output cache dir with LRU GC; ingest on hit, store on miss.~~ Dropped.

## 7. Docs + gates

- [x] 7.1 Document `runtime`/`image` and `kenn init --docker` in the starter
      template comments and any user-facing docs; note docker is a fallback. Done:
      a "Docker indexer runtime" block in `crates/kenn-cli/assets/starter_kenn.toml`
      documents the per-language `runtime = "docker"` + `image` keys, `kenn init
      --docker` (auto-authors the pinned `ghcr.io/kennworx/*` image for a missing
      toolchain), and states docker is a FALLBACK (local preferred); the `--docker`
      flag carries the same in its `--help` text. No `kenn-dotnet` source touched.
- [x] 7.2 Gates: `cargo clippy --workspace --all-targets` (clean), `just crap-ci`
      (PASSED — no regressions, no new over-threshold), `cargo fmt --all` (touched
      only the six edited files); `kenn-dotnet` untouched, so no `dotnet format`.
