## Why

`kenn-embed`'s `LlamaEmbedder::embed` (CC 15) and `llama_backend`
(CC 6) currently have no dedicated test. Their crap-gate status is
held in place by *incidental* coverage: when
`kenn-store/tests/hybrid_search.rs` runs with `KENN_EMBED_MODEL_PATH`
set, the model loads and the embed pipeline runs, and `cargo
llvm-cov` attributes that coverage back to `kenn-embed`. Without the
env var or the model, the same crap-ci run reports both functions
over threshold (crap 240 + 42) and the gate fails — as it does today.

This is fragile in two ways. First, the gate is non-deterministic:
two consecutive `just crap-ci` invocations can disagree depending on
whether the model is cached. Second, the integration that matters —
real GGUF weights producing real vectors through real `llama-cpp-2`
— has no test asserting it works. A `llama-cpp-2` upgrade, a
tokenizer behavior shift, a pooling-type regression, or a model URL
change can land silently; nothing in the suite would catch it.

A single deliberate integration test fixes both: it makes the
coverage attribution deterministic (always runs, when run) and it
turns "the embedder produces sensible vectors" into a checked fact.

## What Changes

- New test file `crates/kenn-embed/tests/llama_integration.rs`
  with one test, `llama_embedder_produces_normalized_vectors`,
  gated `#[cfg(target_os = "macos")]` (matches the `llama-cpp-2`
  dependency gate in `kenn-embed/Cargo.toml`) and marked `#[ignore]`
  (the model download + load is slow and not appropriate for the
  default test pass).
- The test loads `LlamaEmbedder::load()`, calls `.embed(...)` on
  two distinct strings, and asserts: vector count matches input
  count, each vector's length equals the producer's reported
  `dim()`, L2 norm of each vector is approximately 1.0, vectors
  are not all-zero, and the two vectors differ.
- The test calls `release_blocking()` (or drops via `LazyEmbedder`)
  before exiting to avoid the Metal-teardown assert noted at
  `kenn-embed/src/lib.rs:225`.
- New justfile recipe `embed-smoke` that runs the ignored test
  explicitly: `cargo test -p kenn-embed --test llama_integration
  -- --ignored`.

The test is **not** added to the default `cargo test` pass and is
**not** added to CI. It is opt-in via the justfile recipe. The
crap-gate's view of coverage comes from `just crap-ci`, which runs
the full test suite under `cargo llvm-cov`; the question of whether
that run includes ignored tests is addressed in the design.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

(none — this change adds a test that validates the existing
`embedding-producer` requirement "a pluggable embedding producer
turns text into vectors". No requirement text changes.)

## Impact

- **Code**: new file `crates/kenn-embed/tests/llama_integration.rs`,
  one new recipe in `justfile`.
- **CI**: none — the test is `#[ignore]`'d and the recipe is
  developer-facing.
- **Crap-gate**: if `just crap-ci` is updated to include `--ignored`
  on macOS (a design question), `LlamaEmbedder::embed` and
  `llama_backend` exit the over-threshold list deterministically. If
  not, the baseline gains explicit grandfather entries for both
  functions (replacing the dropped entries from commit `85610c8`)
  and the documented path back to coverage cites this test as the
  way to remove them.
- **Dependencies**: no new crates. Test uses `kenn-embed`'s existing
  surface (`LlamaEmbedder`, `EmbeddingProducer`).
- **Disk**: first test run downloads ~300MB GGUF to the local model
  cache. Subsequent runs are offline.
