# CRAP grandfather entries

Per `crap-quality-gate` spec: functions added to `crap-baseline.json`
without test coverage MUST have a record here explaining why coverage
is not reachable in unit-test scope, plus the condition that would let
the entry drop.

When the path-back condition becomes true, the baseline entry SHALL be
removed in the same change that lands the coverage.

## `kenn-embed::llama::llama_backend`

- **File:line**: `crates/kenn-embed/src/llama.rs:37`
- **CC**: 6
- **CRAP at grandfather time**: 42
- **Coverage**: 0%
- **Why unreachable**: `OnceLock<LlamaBackend>` singleton init with a
  double-checked lock. `LlamaBackend::init()` allocates llama.cpp
  process-global state and the `OnceLock` persists across tests within
  one process, so per-test isolation isn't possible without a
  separate-process harness. The branching itself (fast path, gated
  init, error paths) is a small wrapper around the unfaked side
  effect.
- **Path back to coverage**: when one of these becomes true, revisit:
  1. A test harness lands that runs each test in its own subprocess
     (e.g. `nextest` with `--test-threads=1` plus per-test fork).
  2. `llama-cpp` adds a teardown / reset API so the `OnceLock` can be
     re-armed between tests.
  3. The function is refactored to extract the policy decisions
     (locking pattern, error message construction) into a pure helper
     that takes the init outcome as input — the wrapper stays
     uncoverable, the helper covers the branches.

## `kenn-embed::llama::LlamaEmbedder::embed`

- **File:line**: `crates/kenn-embed/src/llama.rs:106`
- **CC**: 15
- **CRAP at grandfather time**: 240
- **Coverage**: 0%
- **Why unreachable** *(in unit scope, not because the function is
  badly written)*: the function tokenizes input, packs sequences into
  `LlamaBatch` up to a token budget, calls `LlamaContext::encode`,
  then pulls embeddings — each step requires a loaded `LlamaModel`
  and an initialized `LlamaBackend`. Surfaced by this change's
  baseline path-rebase (the old baseline lived under a stale worktree
  path and skipped these entries). The CC 15 is mostly batch-packing
  state-machine bookkeeping; the integration test path that runs
  embeddings end-to-end already exists in `kenn-store/tests/hybrid_search.rs`
  (which we just fixed to use `KENN_EMBED_MODEL_PATH`) but `cargo
  llvm-cov` doesn't attribute that coverage back to this function
  because the model isn't always loaded in coverage runs.
- **Path back to coverage**: any of:
  1. Refactor the batch-packing state machine into a pure helper
     `pack_batch(tokenized: &[Vec<Token>], budget: usize) -> Vec<Range<usize>>`
     that returns sequence ranges per decode; that's CC ~6, fully
     unit-testable. The encode loop becomes CC ≤ 5.
  2. Add a kenn-embed integration test using a tiny GGUF fixture that
     can run under llvm-cov without the heavy `LlamaModel::load_from_file`
     cost.

## `kenn-cli::cmd_server::spawn_daemon_and_wait`

- **File:line**: `crates/kenn-cli/src/cmd_server.rs:83`
- **CC**: 6
- **CRAP at grandfather time**: 42
- **Coverage**: 0%
- **Why unreachable**: actually spawns a child OS process via
  `Command::new(exe).spawn()` and polls `/healthz` against the child's
  bound port. Testing the wait loop without spawning requires injecting
  a `CommandFactory` trait + a `HealthzProber` trait pair — a DI
  refactor larger than the function it wraps. The polled `probe_healthz`
  helper already lives in `kenn-embed`; threading it through a trait
  hierarchy for one CLI entrypoint isn't proportionate.
- **Path back to coverage**: when one of these becomes true, revisit:
  1. A CLI integration-test harness lands that spawns `kenn server`
     in a subprocess and observes `/healthz` end-to-end — those tests
     cover this function by construction and the baseline entry can
     drop.
  2. The function's spawn + wait split is genuinely separated (e.g.
     `spawn_child()` + `wait_for_health(prober, deadline)`); the wait
     half becomes a pure function of the prober trait and is testable
     against a mock.
