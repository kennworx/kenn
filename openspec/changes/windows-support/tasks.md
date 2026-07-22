## 1. Stop shipping a broken target

- [x] 1.1 Remove `x86_64-pc-windows-msvc` from `targets` in
  `dist-workspace.toml`, run `dist generate`, and confirm with
  `dist plan --output-format=json` that only the three working targets
  remain. → verify: a tag builds and publishes a release, and the
  Homebrew formula updates (that job runs after the full matrix, so it
  is currently blocked by the Windows failure).
  Done: matrix is macos-14 / ubuntu-24.04-arm / ubuntu-22.04. The
  powershell installer went with it — nothing left for it to install.
- [x] 1.2 Note in `docs/releasing.md` that Windows is intentionally absent
  and what gates its return.

## 2. Make the workspace compile on Windows

- [x] 2.1 Add a `windows-2022` job running
  `cargo check --workspace --all-targets`. Land it BEFORE the fixes so
  the failure is visible in CI first. → verify: the job fails on the two
  known `kenn-store` errors (E0433, E0599).
  Authored `.github/workflows/ci-windows.yml` (windows-2022, stable
  toolchain, LLVM for the bindgen libclang, `cargo check --workspace
  --all-targets`); actionlint-clean. NOTE: the fixes land in the SAME
  increment, so the "see the failure first" ordering was not followed —
  the CI run confirms the fixed state, not the E0433/E0599 baseline.
  Unverified until a real Windows runner executes it (D5: green locally
  says nothing about Windows).
- [x] 2.2 Give `ancestor_device_id`
  (`crates/kenn-store/src/layout/resolve.rs:63`) a Windows implementation
  comparing canonicalised volume prefixes (D4), keeping the unix
  `st_dev` path. → verify: the CI job's two errors are gone.
  Done: `ancestor_device_id` is `cfg`-split — unix `st_dev`, and a
  `cfg(not(unix))` arm that hashes the canonicalised volume prefix
  (drive letter / UNC share) to a `u64`, so `same_filesystem` is
  unchanged. Stable std only. macOS clippy + tests green (unix arm); the
  Windows arm only compiles under the CI gate.
- [x] 2.3 Replace `std::os::unix::fs::symlink` in
  `crates/kenn-store/src/layout/store.rs:162` and
  `crates/kenn-store/src/lifecycle/tests.rs:212` with whatever task 3
  introduces. → verify: `cargo check` green on all four platforms.
  Done (pointer-file writes). The §6 audit turned up more `os::unix`
  symlink sites the two named ones missed: `cmd_index/core.rs`
  (`atlas_pointer`, already `cfg(unix)`-gated but semantically broken),
  and three integration tests (`kenn-store/tests/findings.rs`,
  `kenn-mcp/tests/{background_reindex,findings_tools}.rs`) — all now
  write the pointer. macOS `cargo check` green; the four-platform claim
  needs the CI gate.

## 3. Convert `live` from a symlink to a pointer file

- [x] 3.1 Collapse the duplicated readers: `Store::live_target`
  (`layout/store.rs:96`) delegates to `Layout::live_target`
  (`layout/types.rs:246`) (D2). → verify: existing tests pass with one
  `read_link` call site remaining, not two.
  Done — `Store::live_target` is now `self.layout.live_target()`; one
  reader.
- [x] 3.2 Change that single reader to read the pointer file. → verify:
  a store whose `live` is a symlink resolves to `None` rather than
  panicking or following it (D3).
  Done — `read_to_string` + trim + empty-guard, resolve relative to
  `derived_root`. An old-store symlink → `read_to_string` follows to a
  dir → errs → `None` (D3 degrade). Covered by the passing
  `first_time_init_has_no_live` / `steady_state_layout_after_one_run`.
- [x] 3.3 Change `atomic_flip_live`
  (`lifecycle/atomic.rs:10`) to write the relative target to a temp file
  and rename it over `live`, and DELETE the `cfg(not(unix))` error
  branch — one code path, no fork (D1). → verify: `kenn index` then
  `kenn status` reports the new run; `kenn rollback` reports the prior
  one.
  Done — write-temp-fsync-then-`rename`, `cfg` fork gone. Verified via
  `begin_then_publish_flips_live` and `rollback_walks_back_one_run`
  (live_target reports the new / prior run). End-to-end `kenn index`
  smoke not re-run this session.
- [x] 3.4 Update the tests that assert symlink mechanics:
  `worktree.rs:316,328` (`symlink_metadata`), `lifecycle/tests.rs:212`,
  and `layout/store.rs:165` (`assert!(...is_symlink())` — that one
  inverts to asserting `live` is a regular file). → verify: they assert
  pointer-file behaviour and still fail when the behaviour is broken.
  Done — `worktree.rs` uses `fs::metadata`; `store.rs` asserts
  `is_file()` + `!is_symlink()`; `tests.rs` gc site writes the pointer.
- [x] 3.5 Retry the flip on a Windows sharing violation — a few attempts
  with short backoff — and surface a named failure if they are exhausted
  (D6). Safe because the temp file already holds the complete target, so
  the operation is idempotent. → verify: a failed flip reports a failed
  publish; it does NOT report success while `kenn status` still shows the
  previous run.
  Done — `rename_pointer` retries up to 5× (20/40/…/100 ms) only on
  `ERROR_SHARING_VIOLATION` (raw os error 32), then returns the error.
  Platform-neutral (POSIX never returns 32 → one attempt, no cfg fork).
  The exhaustion path only fires on Windows; not exercisable on macOS.

## 4. Re-establish the concurrency guarantee

- [x] 4.1 Rewrite the concurrent-reader test
  (`lifecycle/tests.rs:317-409`) against the pointer file: a reader
  thread resolving `live` across a flip must never see an absent, empty,
  or truncated pointer.
  Done — `live_pointer_repoint_is_atomic_for_realistic_readers`:
  `read_to_string` + empty-guard, asserts no failed/empty read and every
  observed pointer is one of the two valid relative paths. Passes.
- [x] 4.2 **Mutation-check it** — write the pointer in place instead of
  via rename and confirm the test goes RED, then restore. A concurrency
  test that has never failed is not evidence of anything. → verify: red
  on the mutation, green on restore.
  Done — non-atomic in-place write (`File::create(live)` + 3 ms + write)
  made the test FAIL ("observed 3 failed/empty pointer read(s); first
  error: empty/truncated pointer"); restored → green.

## 5. Docker runtime is unsupported on Windows

- [x] 5.1 Make `kenn init --docker` on Windows decline the docker runtime
  and say why, naming local toolchains and WSL2. → verify: run
  `kenn init --docker` on the Windows CI job and assert the output.
  Done — `containerize_decision` (cmd_init.rs) takes a `windows: bool` and, on
  `(opt_in, windows=true, _)`, declines with a message naming local toolchains
  and WSL2; the caller passes `cfg!(windows)` and `docker && !windows` short-
  circuits the daemon probe so a Windows run never spawns `docker info`. The
  pure function is unit-tested on macOS by passing `windows=true`
  (`containerize_decision_cases`). The live `kenn init --docker` output assert
  on a real Windows runner is the CI half, still pending.
- [x] 5.2 State the limitation in `docker/README.md` and the main
  `README.md`.
  Done — `docker/README.md` gained a "Platform support" section (images are
  Linux-only; Windows → local toolchains or Docker Desktop + WSL2), and
  `README.md` notes it beside the docker-runtime explanation.

## 6. Confirm nothing else assumed a symlink

- [x] 6.1 Search for anything outside kenn-store that treats `live` as
  traversable (`cd live`, `live/...` path joins, symlink-aware walks).
  `staleness.rs:185` uses `symlink_metadata` for source files rather
  than `live` — confirm rather than assume (design risk note).
  Done — and the proposal's "nothing traverses `live`" claim was WRONG.
  `cmd_index/core.rs` `atlas_pointer` symlinked `.kenn/atlas ->
  local/live/atlas` (a string literal, so the proposal's `live_path().join`
  grep missed it) — that path traverses `live` and dangles on a pointer
  file; fixed to resolve `live_target()` and link at the run's atlas dir.
  `staleness.rs:185` confirmed harmless (walks SOURCE files, not `live`).
- [x] 6.2 Check the MCP watcher (`crates/kenn-mcp/src/watcher.rs:214`)
  and `kenn docker-cache`/atlas paths still resolve the live run.
  → verify: `kenn mcp` startup serves the live snapshot on a fresh
  index.
  Done by inspection — the watcher watches the `live` *path* for write
  events (a pointer rename fires the same event a symlink flip did), then
  reads via `live_target()`; no symlink assumption. `atlas_pointer` fixed
  (6.1). MCP integration tests that seed a `live` fixture were converted
  to pointer writes. `kenn mcp` startup smoke not re-run this session.

## 7. Return Windows to the release matrix

- [x] 7.1 Restore `x86_64-pc-windows-msvc` to `dist-workspace.toml`,
  `dist generate`, and confirm the runner mapping with `dist plan`.
  Done — added the target (+ restored the `powershell` installer that went with
  it), `dist generate` regenerated `.github/workflows/release.yml`, and
  `dist plan` lists the 4th artifact `kenn-x86_64-pc-windows-msvc.zip`. The
  prerequisite ci-windows gate is GREEN — but note task 2 undercounted: the gate
  surfaced that **kenn-server** (pid.rs/runtime.rs) also blocked Windows (stale
  windows-sys FFI: `handle == 0` on a now-`*mut c_void` HANDLE, and 5 `unsafe`
  blocks lacking the `deny(unsafe_code)` opt-out) — both fixed and CI-confirmed.
- [ ] 7.2 Tag a release and confirm a Windows artifact is produced AND
  the Homebrew formula still publishes. → verify: the release page lists
  a Windows archive; `Formula/kenn.rb` is updated in the tap.
- [ ] 7.3 Manually smoke `kenn init` → `kenn index` → `kenn status` on a
  real Windows machine as an unelevated user. CI `cargo check` proves
  compilation, NOT that indexing works — the flip bug compiled fine.
