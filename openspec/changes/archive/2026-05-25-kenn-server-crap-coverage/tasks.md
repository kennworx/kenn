## 1. Audit current shape

- [x] 1.1 Re-read `kenn-cli/src/cmd_server.rs::status` (line 192) and
  confirm the five-branch match shape on `(responsive, pid,
  cleaned_stale)`. Identify the data dependencies (`config.server.addr`,
  `pid_file()`, `runtime::status()`, `probe_healthz`) and confirm
  none of them already have a "fake/mock" variant we could reuse.
- [x] 1.2 Re-read `kenn-cli/src/cmd_server.rs::start` (line 33).
  Confirm the dispatcher's three branches (foreground, from_handoff,
  spawn-daemon) and that all interesting side effects
  (`serve_until_shutdown`, `daemonize`, `spawn_daemon_and_wait`) live
  in callees, not inline.
- [x] 1.3 Re-read `kenn-cli/src/cmd_server.rs::spawn_daemon_and_wait`
  (line 65) and `kenn-embed/src/llama.rs::llama_backend` (line 37).
  Confirm both are dominated by single side-effects (process spawn /
  OnceLock singleton init) such that test coverage requires a
  structural refactor disproportionate to the function size.

## 2. `cmd_server::status` — extract + test

- [x] 2.1 Extract `fn render_status(addr: &str, responsive: bool,
  status: &RuntimeStatus) -> String` (or equivalent) into
  `cmd_server.rs`. The function returns the printed line; no
  filesystem or network access. Inputs:
  - `addr` — `config.server.addr` (already a `&str`)
  - `responsive` — `probe_healthz` result
  - `status` — `RuntimeStatus` struct with `pid: Option<u32>` and
    `cleaned_stale: bool`
- [x] 2.2 Rewrite `fn status() -> Result<ExitCodes>` as a thin
  orchestrator: resolve dependencies → call `render_status` → print →
  return. Target CC ≤ 3.
- [x] 2.3 Add `render_status_table` unit test in `cmd_server.rs` with
  test rows for each branch:
  - `(true, Some(42))` → "running (pid 42, healthy)"
  - `(true, None)` → "running externally (responded to /healthz; no local PID file)"
  - `(false, Some(42))` → "pid 42 alive but /healthz unreachable (unresponsive)"
  - `(false, None, cleaned_stale=true)` → "not running (stale PID file cleaned up)"
  - `(false, None, cleaned_stale=false)` → "not running"
- [x] 2.4 Confirm `status` drops below CRAP 30 (target: CC 3, cov 100%
  on the rendered shell, render helper at CC ~5 cov 100% → CRAP < 10
  each).

## 3. `cmd_server::start` — extract + test

- [x] 3.1 Extract:
  ```rust
  enum StartMode { ForegroundDirect, ForegroundFromHandoff, SpawnDaemon }
  fn decide_start_mode(foreground: bool, from_handoff: bool) -> StartMode
  ```
- [x] 3.2 Rewrite `fn start(foreground, idle_timeout)` to use
  `decide_start_mode`, then match on the returned enum. Each arm
  invokes the relevant side-effecting function as today. Target
  CC ≤ 4.
- [x] 3.3 Add `decide_start_mode_table` unit test for all four
  `(foreground: bool, from_handoff: bool)` combinations.
- [x] 3.4 Confirm `start` drops below CRAP 30 (wrapper at CC 4, cov
  0% → CRAP 20; helper at CC 4, cov 100% → CRAP < 5).

## 4. `llama_backend` — grandfather

- [x] 4.1 Add `openspec/changes/kenn-server-crap-coverage/crap-grandfather.md`
  with an entry for `llama_backend`:
  - file:line, CC, CRAP at grandfather time
  - reason: `OnceLock<LlamaBackend>` singleton init with double-checked
    lock; `LlamaBackend::init()` allocates llama.cpp state and the
    `OnceLock` persists across tests in one process, so per-test
    isolation requires a separate-process harness
  - path-back-to-coverage: if a future test harness runs each test in
    its own process (or llama.cpp adds a teardown API), revisit
- [x] 4.2 Covered by §8.2 (single `just crap-baseline` run AFTER all
  refactor + grandfather work writes the entry alongside the others).

## 5. `spawn_daemon_and_wait` — grandfather

- [x] 5.1 Add `spawn_daemon_and_wait` to the same
  `crap-grandfather.md`:
  - file:line, CC, CRAP at grandfather time
  - reason: actually spawns a child OS process via
    `Command::new(exe).spawn()`; testing the wait loop without
    spawning requires injecting a `CommandFactory` + `HealthzProber`
    trait pair, which is a larger refactor than the function itself
  - path-back-to-coverage: if/when a CLI integration-test harness lands
    that spawns kenn server in subprocess and observes /healthz, this
    function becomes covered by those tests and the baseline entry
    can drop
- [x] 5.2 Covered by §8.2 (same baseline regeneration).

## 6. `resolve_model_path` — extract + test

(Surfaced by the baseline path-rebase that this change performs;
pre-existing in `kenn-embed` but hidden from `just crap-ci` by the
stale `.worktrees/extract-server/` path prefix in the old baseline.)

- [x] 6.1 In `kenn-embed/src/llama.rs`, extract a pure classifier:
  ```rust
  enum ModelPathOutcome { UseExplicit(PathBuf), ExplicitMissing(PathBuf),
                          UseCache(PathBuf), Download(PathBuf) }
  fn classify_model_path(
      explicit: Option<&str>,
      cache_path: PathBuf,
      file_exists: impl Fn(&Path) -> bool,
  ) -> ModelPathOutcome
  ```
- [x] 6.2 Extract `apply_model_path_outcome(outcome, cache_dir)` for
  the side-effecting tail so the wrapper stays at CC ≤ 3.
- [x] 6.3 Rewrite `resolve_model_path` to: read env, resolve cache
  dir, call classifier, call applier.
- [x] 6.4 Tests:
  - `classify_model_path_*` (5 tests, one per outcome including
    empty-env-treated-as-unset).
  - `apply_outcome_returns_path_for_use_arms` and
    `apply_outcome_errors_on_explicit_missing` for the testable arms
    (the `Download` arm is filesystem + network; not unit-tested).
- [x] 6.5 Confirm `resolve_model_path` drops out of the baseline.

## 7. `LlamaEmbedder::embed` — grandfather

(Same baseline path-rebase finding as §6 — pre-existing CC 15
pipeline, no test coverage attributed by `cargo llvm-cov`.)

- [x] 7.1 Add `LlamaEmbedder::embed` to `crap-grandfather.md`:
  - file:line, CC 15, CRAP 240, coverage 0%
  - reason: pipeline requires loaded `LlamaModel` + initialized
    `LlamaBackend`; integration test exists in
    `kenn-store/tests/hybrid_search.rs` but llvm-cov doesn't attribute
    that coverage back to this function in coverage runs
  - path back to coverage: extract the batch-packing state machine
    into a pure helper, OR add a kenn-embed integration test with a
    tiny GGUF fixture that runs under llvm-cov
- [x] 7.2 Covered by §8.2 (single baseline regeneration writes all
  grandfathered entries).

## 8. Spec-side touch (crap-grandfather convention)

- [x] 8.1 The added requirement in `specs/crap-quality-gate/spec.md`
  is satisfied by §4.1 + §5.1 + §7.1. Confirm the `crap-grandfather.md`
  file format is consistent and easy to grep (one section per
  function, function name as `##` header).
- [x] 8.2 Regenerate `crap-baseline.json` via `just crap-baseline`
  AFTER all refactor work in §2/§3/§6 has landed and all grandfather
  entries in §4/§5/§7 are documented, so the baseline diff is exactly
  the expected change (three grandfathered entries, three removed
  refactored-below-threshold entries).

## 9. Verification

- [x] 9.1 `cargo clippy --workspace --all-targets` clean.
- [x] 9.2 `cargo test --workspace` clean.
- [x] 9.3 `just crap-ci` passes with exit 0 against the updated
  baseline.
- [x] 9.4 Inspect the baseline diff: it should add exactly three new
  grandfathered entries (`llama_backend`, `spawn_daemon_and_wait`,
  `LlamaEmbedder::embed`) and zero regressions or unexpected newcomers.
- [x] 9.5 `openspec validate kenn-server-crap-coverage` clean.
