## Why

After `extract-kenn-server` landed (commits `561b42d` / `6bce8d3`) and
`embedding-model-update` work touched `kenn-embed`, six functions appear
over the CRAP threshold (30). All six are at coverage 0%; the
cyclomatic complexity is mostly modest (6–8, plus one outlier at 15)
but the cube of `1 − coverage` drives CRAP into the red:

| File | Function | CC | Coverage | CRAP |
|---|---|---:|---:|---:|
| `kenn-cli/src/cmd_server.rs:33` | `start` | 7 | 0% | 56 |
| `kenn-cli/src/cmd_server.rs:65` | `spawn_daemon_and_wait` | 6 | 0% | 42 |
| `kenn-cli/src/cmd_server.rs:192` | `status` | 8 | 0% | 72 |
| `kenn-embed/src/llama.rs:37` | `llama_backend` | 6 | 0% | 42 |
| `kenn-embed/src/llama.rs:106` | `LlamaEmbedder::embed` | 15 | 0% | 240 |
| `kenn-embed/src/llama.rs:226` | `resolve_model_path` | 8 | 0% | 72 |

The first four were not in scope for `crap-complexity-refactor` (which
targets the original 33-function offender list from before the daemon
work). The last two (`LlamaEmbedder::embed`, `resolve_model_path`) were
pre-existing in `kenn-embed` but hidden from `just crap-ci`: the
checked-in `crap-baseline.json` lived under a stale
`.worktrees/extract-server/` path prefix and the gate's path-match
silently skipped any non-matching entries. Regenerating the baseline as
part of this change exposes them.

After reading each function, the six split cleanly into two buckets:

| Function | Bucket | Why |
|---|---|---|
| `status` | **Refactorable + testable** | Orchestrator that resolves config / pid-path / healthz, then formats one of five lines from `(responsive, pid, cleaned_stale)`; extract the formatter into a pure `render_status` helper that takes resolved inputs — the orchestrator keeps the dependency resolution, the helper is trivially table-testable |
| `start` | **Refactorable + testable** | Env-var + flag dispatcher around `serve_until_shutdown`; extract the dispatch decision into a pure helper, leave the blocking side-effect at the orchestrator boundary |
| `resolve_model_path` | **Refactorable + testable** | Env + cache file probe + download branching; extract a pure classifier that takes an injected `file_exists` predicate and returns a `ModelPathOutcome` enum |
| `spawn_daemon_and_wait` | **Grandfather** | Actually `Command::new(exe).spawn()` a real OS process; testing without spawning needs a full Command-factory + healthz-prober DI refactor that's larger than the function itself |
| `llama_backend` | **Grandfather** | `OnceLock<LlamaBackend>` global-singleton init with a double-checked lock; init is the side-effect (allocates llama.cpp state); `OnceLock` state persists across tests in one process so per-test isolation isn't possible without a separate-process harness |
| `LlamaEmbedder::embed` | **Grandfather** | CC 15 tokenize → batch-pack → encode → read embeddings pipeline; each step requires a loaded `LlamaModel`. Surfaced by the baseline path-rebase. Refactor path documented (extract pure `pack_batch` state machine) but out of this change's scope |

The spec's principle holds: add coverage when reachable, grandfather only
when genuinely uncoverable AND document why. The three refactorable
functions get tests; the three uncoverable ones get explicit baseline
entries with the rationale recorded next to them.

## What Changes

### `cmd_server::status` — refactor + tests (tractable)

Today `status()` is a 28-line orchestrator that resolves config, the
healthz URL, the pid-file path, and the runtime status, then prints
one of five branches based on `(responsive, pid)` + the
`cleaned_stale` flag.

Refactor:

- Extract the pure decision kernel:
  ```rust
  fn render_status(url: &str, responsive: bool, s: &kenn_server::runtime::Status) -> String
  ```
  This takes resolved inputs and returns the printed line; it has no
  filesystem or network calls and is trivially table-testable across
  all five branches.
- The thin orchestrator (`fn status()` proper) keeps the dependency
  resolution, calls `render_status`, prints. Its CC drops to ≤ 3 (it
  becomes a straight-line resolve → call → println sequence with one
  `?` per resolution step).

Tests: a single `render_status_table` that walks all five
`(responsive, pid, cleaned_stale)` tuples and asserts the produced
string.

Expected outcome: `status` drops from CRAP 72 to a coverable shell at
CC ≤ 3, and `render_status` enters at CC ~5 with full coverage → CRAP
under 10.

### `cmd_server::start` — small extract + tests (tractable)

Today `start()` is a 23-line env-var dispatcher whose three branches
all call blocking side-effecting code (`serve_until_shutdown`,
`daemonize`, `spawn_daemon_and_wait`).

Refactor:

- Extract a pure decision helper:
  ```rust
  enum StartMode { ForegroundDirect, ForegroundFromHandoff, SpawnDaemon }
  fn decide_start_mode(foreground: bool, from_handoff: bool) -> StartMode
  ```
- Extract a `run_foreground(idle_timeout, daemonize: bool)` helper that
  collapses the two foreground arms into one body — needed because the
  match arms still contain inline `?` operators and tokio-runtime
  setup, which keeps the wrapper's CC above threshold on their own.
- `start()` reduces to: `decide_start_mode(...)` → match arm → one
  function call per arm.

Tests: `decide_start_mode_table` covers the four `(foreground,
from_handoff)` combinations.

Expected outcome: the wrapper drops to CC ≤ 3 (one-call-per-arm match)
and remains uncovered (the side-effecting arms can't be unit-tested);
the extracted helper covers the branching logic.

### `llama::resolve_model_path` — extract + tests (tractable)

Today `resolve_model_path()` resolves the model GGUF path in three
steps: read `KENN_EMBED_MODEL_PATH`, fall back to the cache, fall back
to a one-time download. The four possible outcomes (explicit-hit,
explicit-missing-error, cache-hit, download) are interleaved with the
filesystem probes that decide between them.

Refactor:

- Extract a pure classifier:
  ```rust
  enum ModelPathOutcome { UseExplicit(PathBuf), ExplicitMissing(PathBuf),
                          UseCache(PathBuf), Download(PathBuf) }
  fn classify_model_path(
      explicit: Option<&str>,
      cache_path: PathBuf,
      file_exists: impl Fn(&Path) -> bool,
  ) -> ModelPathOutcome
  ```
- Extract `apply_model_path_outcome(outcome, cache_dir)` which turns
  the outcome into either the resolved `PathBuf` or the download
  side-effect. Three of its four arms are unit-testable (the Download
  arm calls real `std::fs::create_dir_all` + network download; the
  other three are pure).
- `resolve_model_path()` becomes a thin orchestrator: read env, resolve
  cache dir, call classifier, call applier. CC ≤ 3.

Tests:
- `classify_model_path_*` (5 tests) for each outcome variant including
  the empty-env-treated-as-unset corner.
- `apply_outcome_returns_path_for_use_arms`,
  `apply_outcome_errors_on_explicit_missing` for the testable arms.

Expected outcome: `resolve_model_path` drops out of the baseline.

### `llama_backend` — grandfather with rationale

`llama_backend()` is a `OnceLock<LlamaBackend>` singleton init with a
double-checked lock. It is unsafe-by-design to call twice; the
`LlamaBackend::init()` call is the entire side-effect.

Add to `crap-baseline.json` with an explicit `reason` comment in a
companion `crap-grandfather.md` listing the function, the CRAP score
at acceptance, and the testability blocker (singleton init shares
process state). Future changes that bring `llama_backend` down to
threshold should remove its baseline entry.

### `spawn_daemon_and_wait` — grandfather with rationale

`spawn_daemon_and_wait` runs `Command::new(exe).spawn()` to launch a
child kenn process. Testing the wait loop without actually spawning
requires injecting a `CommandFactory` trait + a `HealthzProber` trait;
that DI refactor is larger than the function it wraps and isn't
clearly worth the cost for a daemon entrypoint that's also exercised
by manual smoke + the eventual CLI integration tests.

Grandfather as above, with the same `crap-grandfather.md` entry.

### `LlamaEmbedder::embed` — grandfather with rationale

`LlamaEmbedder::embed` is a CC 15 tokenize → batch-pack → encode →
read-embeddings pipeline that requires a loaded `LlamaModel` and an
initialized `LlamaBackend` to run any branch. Surfaced by this
change's baseline path-rebase (it existed in the source before but the
`.worktrees/extract-server/` path prefix in the old baseline silently
skipped it). Refactor path is documented in `crap-grandfather.md`:
extract a pure `pack_batch(tokenized, budget)` state machine and the
encode loop drops to CC ≤ 5. Out of this change's scope; the entry is
grandfathered with the same convention as the other two.

### Verification

`just crap-ci` MUST report exit 0. The refactor work brings `status`,
`start`, and `resolve_model_path` under threshold; the grandfather
entries quiet `llama_backend`, `spawn_daemon_and_wait`, and
`LlamaEmbedder::embed` without expanding what can be silently
accepted in the future.

## Capabilities

### Modified Capabilities

- `crap-quality-gate`: clarifies the criteria for grandfathering an
  over-threshold function in the baseline. Today the spec implies
  "always add coverage"; this change adds explicit recognition that
  certain functions (singleton inits, process-spawn entrypoints) are
  genuinely uncoverable in unit scope and require either a larger
  structural refactor OR a documented baseline grandfather decision.
  Adds a `crap-grandfather.md` convention where each grandfathered
  entry carries the reason and the path back to coverage.
