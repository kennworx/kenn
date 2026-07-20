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

> **Status: postponed after task 1.** Tasks 2-7 are not started. The
> release matrix no longer lists Windows, so nothing here is blocking;
> pick this up when Windows support is actually wanted.

## 2. Make the workspace compile on Windows

- [ ] 2.1 Add a `windows-2022` job running
  `cargo check --workspace --all-targets`. Land it BEFORE the fixes so
  the failure is visible in CI first. → verify: the job fails on the two
  known `kenn-store` errors (E0433, E0599).
- [ ] 2.2 Give `ancestor_device_id`
  (`crates/kenn-store/src/layout/resolve.rs:63`) a Windows implementation
  comparing canonicalised volume prefixes (D4), keeping the unix
  `st_dev` path. → verify: the CI job's two errors are gone.
- [ ] 2.3 Replace `std::os::unix::fs::symlink` in
  `crates/kenn-store/src/layout/store.rs:162` and
  `crates/kenn-store/src/lifecycle/tests.rs:212` with whatever task 3
  introduces. → verify: `cargo check` green on all four platforms.

## 3. Convert `live` from a symlink to a pointer file

- [ ] 3.1 Collapse the duplicated readers: `Store::live_target`
  (`layout/store.rs:96`) delegates to `Layout::live_target`
  (`layout/types.rs:246`) (D2). → verify: existing tests pass with one
  `read_link` call site remaining, not two.
- [ ] 3.2 Change that single reader to read the pointer file. → verify:
  a store whose `live` is a symlink resolves to `None` rather than
  panicking or following it (D3).
- [ ] 3.3 Change `atomic_flip_live`
  (`lifecycle/atomic.rs:10`) to write the relative target to a temp file
  and rename it over `live`, and DELETE the `cfg(not(unix))` error
  branch — one code path, no fork (D1). → verify: `kenn index` then
  `kenn status` reports the new run; `kenn rollback` reports the prior
  one.
- [ ] 3.4 Update the tests that assert symlink mechanics:
  `worktree.rs:316,328` (`symlink_metadata`), `lifecycle/tests.rs:212`,
  and `layout/store.rs:165` (`assert!(...is_symlink())` — that one
  inverts to asserting `live` is a regular file). → verify: they assert
  pointer-file behaviour and still fail when the behaviour is broken.
- [ ] 3.5 Retry the flip on a Windows sharing violation — a few attempts
  with short backoff — and surface a named failure if they are exhausted
  (D6). Safe because the temp file already holds the complete target, so
  the operation is idempotent. → verify: a failed flip reports a failed
  publish; it does NOT report success while `kenn status` still shows the
  previous run.

## 4. Re-establish the concurrency guarantee

- [ ] 4.1 Rewrite the concurrent-reader test
  (`lifecycle/tests.rs:317-409`) against the pointer file: a reader
  thread resolving `live` across a flip must never see an absent, empty,
  or truncated pointer.
- [ ] 4.2 **Mutation-check it** — write the pointer in place instead of
  via rename and confirm the test goes RED, then restore. A concurrency
  test that has never failed is not evidence of anything. → verify: red
  on the mutation, green on restore.

## 5. Docker runtime is unsupported on Windows

- [ ] 5.1 Make `kenn init --docker` on Windows decline the docker runtime
  and say why, naming local toolchains and WSL2. → verify: run
  `kenn init --docker` on the Windows CI job and assert the output.
- [ ] 5.2 State the limitation in `docker/README.md` and the main
  `README.md`.

## 6. Confirm nothing else assumed a symlink

- [ ] 6.1 Search for anything outside kenn-store that treats `live` as
  traversable (`cd live`, `live/...` path joins, symlink-aware walks).
  `staleness.rs:185` uses `symlink_metadata` for source files rather
  than `live` — confirm rather than assume (design risk note).
- [ ] 6.2 Check the MCP watcher (`crates/kenn-mcp/src/watcher.rs:214`)
  and `kenn docker-cache`/atlas paths still resolve the live run.
  → verify: `kenn mcp` startup serves the live snapshot on a fresh
  index.

## 7. Return Windows to the release matrix

- [ ] 7.1 Restore `x86_64-pc-windows-msvc` to `dist-workspace.toml`,
  `dist generate`, and confirm the runner mapping with `dist plan`.
- [ ] 7.2 Tag a release and confirm a Windows artifact is produced AND
  the Homebrew formula still publishes. → verify: the release page lists
  a Windows archive; `Formula/kenn.rb` is updated in the tap.
- [ ] 7.3 Manually smoke `kenn init` → `kenn index` → `kenn status` on a
  real Windows machine as an unelevated user. CI `cargo check` proves
  compilation, NOT that indexing works — the flip bug compiled fine.
