## 1. The translated mount

- [ ] 1.1 Add the `MountStrategy::Translate` variant in
  `crates/kenn-indexer/src/docker.rs` and a `docker_launcher` branch for it:
  `-v {win_root}:/work`, `-w /work`, no `--user`, all cache/toolchain volumes
  unchanged. → verify: unit test asserts the argv for `Translate` mounts `/work`
  and omits `--user`, while `SamePath` is byte-for-byte unchanged.
- [ ] 1.2 Settle the Docker Desktop `-v` host-path spelling (`C:\…` vs `C:/…`)
  against a real Docker Desktop (design Decision 5). → verify: `docker run` with
  the chosen form mounts a Windows workspace read/write.
- [ ] 1.3 Select `Translate` on Windows and `SamePath` on POSIX in
  `maybe_docker_command` (replace the hardcoded `MountStrategy::SamePath`). →
  verify: `cfg!(windows)` chooses `Translate`; POSIX unit tests still green.

## 2. Route /work to the driver's workspace-root argument

- [ ] 2.1 Audit how each driver appends the workspace root — jsonl driver
  (kenn-ts, kenn-dotnet) and the SCIP driver (rust/go/python, swift) — and
  confirm the root is the ONLY host-absolute argument (design Decision 2). →
  verify: enumerate the appended args per driver; none other is host-absolute.
- [ ] 2.2 Under `Translate`, make the workspace-root argument `/work` — either by
  `docker_launcher` owning the trailing arg or by each driver emitting `/work`
  when containerized (pick per 2.1). → verify: a containerized Windows invocation
  passes `/work`, not the host path.
- [ ] 2.3 **Reconcile `metadata.project_root` at ingest** (design Decision 2b):
  under docker+Translate, map the container mount point (`/work`) to the host
  workspace root in `pipeline/ingest.rs` (`absorb_scip_metadata` /
  `project_root_uri`) before canonicalization — driven by the runtime signal, not
  by sniffing the path. Without this, `canonicalize` drops every record as
  `OutsideRoot` and the index is silently empty. → verify: a test with
  `project_root = file:///work` + a relative doc canonicalizes INSIDE a host root
  under Translate (records retained), and is dropped WITHOUT the reconciliation —
  break it, watch the index go empty, restore (§9).

## 3. init probes local first, defaults to docker on Windows

- [ ] 3.1 In `crates/kenn-cli/src/cmd_init.rs`, drop the `!windows` guard on
  `daemon_up` so Windows probes the daemon. → verify: on Windows with a runnable
  daemon, `daemon_up` is true.
- [ ] 3.2 Change `containerize_decision` (the GLOBAL `(opt_in, windows,
  daemon_up)` function) so Windows + daemon-up returns `containerize=true`
  WITHOUT opt-in — reorder the arms so the Windows cases precede the blanket
  `(false,_,_) → (false,None)`. Windows + daemon-down returns `(false, None)`
  (degrade path); an explicit `--docker` + daemon-down still errors, naming Docker
  Desktop. POSIX arms unchanged. Also flip the stale docstring ("unsupported on
  Windows"). → verify: extend `containerize_decision_cases` — `(false, true,
  true) → (true, None)`, `(false, true, false) → (false, None)`, `(true, true,
  false) → error naming Docker Desktop`; break each arm, watch red, restore (§9).
- [ ] 3.3 Confirm the per-language routing in `detect_and_classify(ws,
  containerize)` (the EXISTING layer, downstream of 3.2) does the right thing when
  fed `containerize=true` on Windows: a passing local probe → `runtime = "local"`
  (docker never overrides it), a failing probe → `runtime = "docker"`. This layer
  is unchanged; the task is to verify it, not modify it. → verify: a detect test
  with a present-and-working indexer stays local; an absent one becomes docker.
- [ ] 3.4 Change the daemon-absent hint text from "unsupported on Windows" to name
  Docker Desktop (and the local indexer). → verify: the Windows daemon-down hint
  contains "Docker Desktop" and not "unsupported on Windows".

## 4. Cross-change bookkeeping

- [ ] 4.1 When this archives, REMOVE windows-support's requirement "The Docker
  indexer runtime is unsupported on Windows" (windows-platform-support) — this
  change supersedes it. → verify: `openspec validate` clean after archive; no
  contradictory requirement remains.
- [ ] 4.2 Update the `--docker` docs (docker/README.md, README.md) that say docker
  is unsupported on Windows → now supported via Docker Desktop. → verify: docs
  name Docker Desktop as the Windows path.

## 5. Verify end to end

- [ ] 5.1 macOS gate per CLAUDE.md §5–7: `cargo clippy --workspace --all-targets`,
  `just crap-ci`, `cargo fmt --all`. → verify: all green; SamePath behavior and
  its tests unchanged.
- [ ] 5.2 Manual native-Windows smoke on a real host with Docker Desktop: `kenn
  init --docker`, then `kenn index` on a TypeScript repo, a C# repo, and a Swift
  repo; `kenn status` / `kenn get`. → verify: each language is containerized (not
  degraded), symbols are produced, and relative paths resolve — Swift included,
  proving the docker route covers what no native binary could.
- [ ] 5.3 Confirm no reindex/format impact — a workspace indexed via docker on
  Windows stores the same relative canonical paths as any host. → verify: a
  `files.path` sample is `/`-relative and identical in shape to a POSIX index.
