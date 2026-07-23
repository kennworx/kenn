## 1. The translated mount

- [x] 1.1 Add the `MountStrategy::Translate` variant in
  `crates/kenn-indexer/src/docker.rs` and a `docker_launcher` branch for it:
  `-v {win_root}:/work`, `-w /work`, no `--user`, all cache/toolchain volumes
  unchanged. → verify: unit test asserts the argv for `Translate` mounts `/work`
  and omits `--user`, while `SamePath` is byte-for-byte unchanged.
  DONE: `docker_launcher_translate_mounts_work_and_drops_user` +
  `docker_launcher_shared_source_volume_and_ephemeral_build` (SamePath, unchanged).
- [ ] 1.2 Settle the Docker Desktop `-v` host-path spelling (`C:\…` vs `C:/…`)
  against a real Docker Desktop (design Decision 5). → verify: `docker run` with
  the chosen form mounts a Windows workspace read/write.
  CARRIED FORWARD (postponed): needs a live Docker Desktop daemon on Windows. The
  launcher currently emits `{root}:/work` (host path verbatim); confirm/adjust the
  spelling during the 5.2 smoke.
- [x] 1.3 Select `Translate` on Windows and `SamePath` on POSIX in
  `maybe_docker_command` (replace the hardcoded `MountStrategy::SamePath`). →
  verify: `cfg!(windows)` chooses `Translate`; POSIX unit tests still green.
  DONE: `container_mount()` predicate (docker && cfg!(windows)) drives both the
  launcher strategy and (Phase B) the driver arg translation from one source.

## 2. ContainerMount: translate every absolute path arg (host ↔ /work)

- [x] 2.1 DONE — audited every driver's appended args. NOT root-only: kenn-ts
  `--workspace`; kenn-dotnet/kenn-swift `--workspace` + `--projects`×N; rust
  `scip <unit.path>` + `--output`; go `--module-root` + `--output`; python
  `--cwd` + `--target-only` + `--output`. Re-examined derived_root:
  `<ws>/.kenn/local` is inside the workspace, so `/work` covers `--output` — no
  separate mount. (design Decision 2 table.)
- [x] 2.2 Introduce `ContainerMount { host_root }` (`/work` is the `CONTAINER_ROOT`
  const) with `to_container`/`to_host` prefix-swaps, and thread it into the driver
  layer alongside `command` (each driver gains a `mount: Option<ContainerMount>`,
  set from `container_mount(runtime, ws_root)` in workflow.rs). → verify:
  `container_mount_*` tests (incl. `/workspace` boundary + backslash norm) +
  `container_arg_*`. DONE.
- [x] 2.3 Apply `container_arg(self.mount.as_ref(), ..)` at every absolute
  path-arg site in all six drivers — dotnet `--workspace`+`--projects`; ts
  `--workspace` (tsconfigs are relative, left as-is); swift `--workspace`+
  `--projects`; rust `scip <unit>`+`--output`; go `--module-root`+`--output`;
  python `--cwd`+`--output`+`--target-only`. `--output` and the returned SCIP
  `path` stay the HOST path (read back after the run); `CARGO_TARGET_DIR` env left
  alone (the launcher's `-e` overrides it in-container). Discovery stays host-side.
  → verify: `container_arg_*` unit test (Some→/work, None→passthrough); drivers
  spawn subprocesses so per-arg wiring is correct-by-construction + 5.2 smoke. DONE.
- [x] 2.4 **Reconcile `metadata.project_root` at ingest** (design Decision 2b) —
  `reconcile_container_root` in `pipeline/ingest.rs` maps `/work`→host via
  `mount.to_host`, GATED on the runtime signal threaded through
  `ScipDriver::container_mount()` (default `None`; overridden by rust/go/python).
  SCIP-ONLY — the JSONL path emits relative paths, no `project_root`. Gating is
  load-bearing: an early UNCONDITIONAL version regressed
  `scip_documents_outside_the_root_are_counted` (that test uses `/work` as a
  sentinel unrelated root); gating on the mount preserves genuine out-of-root
  signals. → verify: `reconcile_container_root_is_gated_on_the_mount` (Some
  rebases, None leaves `/work` alone) — §9 mutation (identity → red) confirmed;
  the out-of-root regression test is green. DONE.

## 3. init probes local first, defaults to docker on Windows

- [x] 3.1 In `crates/kenn-cli/src/cmd_init.rs`, drop the `!windows` guard on
  `daemon_up` so Windows probes the daemon. → verify: on Windows with a runnable
  daemon, `daemon_up` is true. DONE: `daemon_up = (docker || windows) &&
  daemon_available()` — probes on `--docker` (any host) or Windows (default);
  plain POSIX `init` still short-circuits.
- [x] 3.2 Change `containerize_decision` so Windows + daemon-up returns
  `containerize=true` WITHOUT opt-in (`(true, _, true) | (false, true, true) =>
  (true, None)`); explicit `--docker` + daemon-down errors naming Docker Desktop;
  everything else degrades (`(false, _, _)`). Docstring flipped. → verify:
  `containerize_decision_cases` covers all 8 combos; §9 mutation ((false,true,true)
  → not-containerize) confirmed red then restored. DONE.
- [x] 3.3 Confirm the per-language routing in `detect_and_classify(ws,
  containerize)` (EXISTING layer) routes probe-pass → `local`, probe-fail +
  containerize → `docker`, probe-fail + !containerize → text. VERIFIED via the
  `Classification` doc + existing behavior (detect.rs ~238-242) — unchanged; feeding
  `containerize=true` on Windows yields the probe-first result. No code change.
- [x] 3.4 Change the daemon-absent hint text from "unsupported on Windows" to name
  Docker Desktop (and the local indexer). → verify: the Windows daemon-down hint
  contains "Docker Desktop" and not "unsupported on Windows". DONE: the
  `(true, true, false)` arm names Docker Desktop; asserted in the test; old string
  gone. (Silent-degrade case emits no global message — per-language hint carries it.)

## 4. Cross-change bookkeeping

- [x] 4.1 When this archives, REMOVE windows-support's requirement "The Docker
  indexer runtime is unsupported on Windows" (windows-platform-support) — this
  change supersedes it. → verify: `openspec validate` clean after archive; no
  contradictory requirement remains. DONE: authored
  `specs/windows-platform-support/spec.md` with a `## REMOVED Requirements` delta;
  archive applies the removal.
- [x] 4.2 Update the `--docker` docs (docker/README.md, README.md) that say docker
  is unsupported on Windows → now supported via Docker Desktop. → verify: docs
  name Docker Desktop as the Windows path. DONE: both rewritten — docker is the
  default Windows path via Docker Desktop; docker/README documents the `/work`
  Translate mount.

## 5. Verify end to end

- [x] 5.1 macOS gate per CLAUDE.md §5–7: `cargo clippy --workspace --all-targets`,
  `just crap-ci`, `cargo fmt --all`. → verify: all green; SamePath behavior and
  its tests unchanged. DONE: workspace clippy clean, CRAP gate PASSED (no
  regressions / no new over-threshold), fmt applied (only edited files). Windows
  cross-check walls locally on `ring`'s C build (no Windows headers on this Mac) —
  no new cfg-gated code, so macOS compiles every line; Windows compile is gated by
  `ci-windows.yml` on push.
- [ ] 5.2 Manual native-Windows smoke on a real host with Docker Desktop: `kenn
  init --docker`, then `kenn index` on a TypeScript repo, a C# repo, and a Swift
  repo; `kenn status` / `kenn get`. → verify: each language is containerized (not
  degraded), symbols are produced, and relative paths resolve — Swift included,
  proving the docker route covers what no native binary could.
  CARRIED FORWARD (postponed, unverified at archive): needs a real Windows host
  with Docker Desktop, not runnable in-sandbox. `ci-windows` proves the change
  COMPILES on Windows; it does NOT prove `docker run` with the `/work` mount
  actually indexes — that is this smoke. Settle task 1.2's `-v` spelling here too.
- [x] 5.3 Confirm no reindex/format impact — a workspace indexed via docker on
  Windows stores the same relative canonical paths as any host. → verify: a
  `files.path` sample is `/`-relative and identical in shape to a POSIX index.
  DONE (by construction): `container_mount` returns `None` on non-Windows, so POSIX
  output is byte-identical (all existing tests unchanged); no store schema or wire
  change; `WorkspaceRelativePath` is `/`-canonical on every OS.
