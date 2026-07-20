## Context

`kenn-embed` is the embedding boundary; `LlamaEmbedder` is its
in-process implementation backed by `llama-cpp-2`. The crate is
mac-only — `llama-cpp-2` appears under `[target.'cfg(target_os =
"macos")'.dependencies]` and the file `crates/kenn-embed/src/llama.rs`
does not compile on Linux/Windows. Today there is no test directly
exercising `LlamaEmbedder::embed`. Indirect coverage exists in
`crates/kenn-store/tests/hybrid_search.rs`, which skips when
`KENN_EMBED_MODEL_PATH` is unset.

The crap-gate's reading of these functions has drifted: at commit
`85610c8` they sat below threshold in the published baseline; on
`main` today (`be1d37b`) `just crap-ci` fails on
`LlamaEmbedder::embed` (crap 240) and `llama_backend` (crap 42).
The same source code, different coverage attribution — the gate is
non-deterministic.

The `crap-grandfather.md` record from `kenn-server-crap-coverage`
documents a "path back to coverage" for both functions: option (1)
is a `pack_batch` extraction (refactor), option (2) is "a kenn-embed
integration test using a tiny GGUF fixture that can run under
llvm-cov." This change pursues option (2) but with the real model
rather than a fixture (see Decisions).

## Goals / Non-Goals

**Goals:**
- One mac-gated integration test that exercises `LlamaEmbedder::load`
  + `.embed(...)` against real `EmbeddingGemma-300M` weights.
- The test asserts outputs are sensible (count, dim, L2 norm, distinctness)
  so a `llama-cpp-2` upgrade or model change that produces nonsense
  fails the test.
- The test is opt-in (`#[ignore]`) and platform-gated
  (`#[cfg(target_os = "macos")]`); no default-suite slowdown, no
  Linux/Windows build break.
- A justfile recipe `embed-smoke` so the test is one command away
  for anyone touching `llama.rs`.

**Non-Goals:**
- Wiring the ignored test into `just crap-ci`. The crap-gate
  question (deterministic gate vs. grandfather entries) is left
  open; see Open Questions.
- A tiny-GGUF test fixture. The model resolver already caches the
  real `embeddinggemma-300M-Q8_0.gguf` after the first download;
  a synthetic mini-GGUF would add maintenance (regenerate when
  `llama-cpp-2` bumps GGUF version) without a clear win.
- Testing `RemoteEmbedder` or the lazy/idle-TTL machinery — those
  are separate concerns with their own coverage stories.
- CI integration on Mac runners. The justfile recipe is enough for
  developer use; CI is a separate decision.
- Refactoring `LlamaEmbedder::embed` (the `pack_batch` extraction
  option from `crap-grandfather.md`). Adding a test does not
  preclude that refactor later; the two are complementary.

## Decisions

### Test lives in `kenn-embed/tests/llama_integration.rs`, not in the existing `hybrid_search.rs`

Two reasons. First, `hybrid_search.rs` lives in `kenn-store` and
tests the whole search pipeline; coverage attribution back to
`kenn-embed::llama` requires `llvm-cov` to traverse the call chain
cleanly, which is exactly the fragile attribution we want to stop
relying on. A test inside `kenn-embed/tests/` is direct: it imports
`kenn_embed::LlamaEmbedder` and calls `.embed(...)` with no
intermediate layers. Second, `hybrid_search.rs` skips silently
without `KENN_EMBED_MODEL_PATH`; the new test should fail loudly if
the model can't load, since the whole point is to assert the load +
embed path works.

Alternative considered: extend `hybrid_search.rs` with an explicit
embed-only test. Rejected because it conflates the kenn-embed-side
contract (does the model produce vectors?) with kenn-store's
contract (does hybrid search rank correctly?). One test per
contract.

### Use the real model, not a fixture

The model resolver (`resolve_model_path`) already downloads and
caches `embeddinggemma-300M-Q8_0.gguf` on first use. Subsequent runs
are offline and fast (model load on M-series is ~1s). A synthetic
mini-GGUF would:

- Need regeneration whenever `llama-cpp-2` upgrades the GGUF
  format version.
- Not catch a `llama-cpp-2` API change in how it consumes our
  specific model architecture (BERT-family, MEAN pooling).
- Not catch the model URL rotting on Hugging Face (the resolver's
  download path stays untested).

The real-model integration is the contract. Test it directly.

### Gating: `#[cfg(target_os = "macos")] #[ignore]`

`#[cfg(target_os = "macos")]` matches the `llama-cpp-2` dep gate in
`Cargo.toml` — the file compiles only where the dep is available.

`#[ignore]` keeps the test out of the default `cargo test`
pass. Justification: a fresh checkout running `cargo test
--workspace` should not download 300MB of model weights, and a CI
worker on Linux shouldn't ever try.

Alternative considered: cargo feature flag (`integration-tests`).
Rejected because the `#[ignore]` + recipe pattern is simpler, more
discoverable (`just --list` shows `embed-smoke`), and matches
existing conventions in the workspace (the `KENN_BENCH` env var
pattern follows the same opt-in shape).

### Test calls `release_blocking()` (or equivalent) before exit

`crates/kenn-embed/src/lib.rs:225` notes that bundled `llama.cpp`
asserts at Metal-device teardown if GPU resources outlive the
device, and Rust `static`s don't drop on their own. The test uses
`LlamaEmbedder` directly (not `LazyEmbedder`), so there's no
process-exit unload to rely on. The test ends by dropping the
embedder explicitly (model is owned by `LlamaEmbedder`, not a
static) — `LlamaModel::drop` should clean up cleanly. We do NOT
call `llama_backend()` cleanup because the `OnceLock<LlamaBackend>`
is process-global and remains live across test runs in the same
process (intentional).

### Assertion shape

```
let v = embedder.embed(&["hello world", "the quick brown fox"]).unwrap();

assert_eq!(v.len(), 2);                              // count
assert_eq!(v[0].len(), embedder.dim());              // dim
assert_eq!(v[1].len(), embedder.dim());              // dim
assert!(l2_norm(&v[0]) > 0.99 && l2_norm(&v[0]) < 1.01);  // normalized
assert!(l2_norm(&v[1]) > 0.99 && l2_norm(&v[1]) < 1.01);  // normalized
assert!(v[0].iter().any(|x| *x != 0.0));             // not all-zero
assert!(v[0] != v[1]);                               // distinct inputs → distinct outputs
```

The assertions catch the broad failure modes:
- Count wrong → batch packing broken.
- Dim wrong → model identity changed or `n_embd` lies.
- Norm not 1 → `l2_normalize` regression or pooling-type mismatch
  produced unnormalized output.
- All-zero → model load succeeded but inference failed silently.
- Identical outputs for distinct inputs → tokenizer collapsed, or
  pooling reads the wrong tensor.

A semantic-similarity assertion (e.g. "the quick brown fox" closer
to "a fast brown animal" than to "the price of tea") is tempting
but out of scope. The current assertions catch structural failure,
not semantic drift.

## Risks / Trade-offs

- **300MB first-run download.** Mitigated by `#[ignore]` (opt-in)
  and by the existing resolver caching it under the local model
  cache for reuse.
- **Metal init / teardown noise.** First test invocation in a new
  process initializes the global backend; subsequent invocations
  reuse it via `OnceLock`. No mitigation needed — the singleton
  semantics are the intended design.
- **The test fails offline if the model isn't cached.** Acceptable:
  the test is opt-in, and the developer running `just embed-smoke`
  is asking for a real check.
- **`llama-cpp-2` GGUF format change in a future upgrade breaks the
  cached weights file.** Mitigated by the resolver: when load
  fails, the developer deletes the cache and re-downloads. The
  failure mode is clear, not silent.

## Migration Plan

Not applicable — this is a pure test addition. No migration. No
rollback beyond `git revert`.

## Open Questions

- **Should `just crap-ci` invoke the ignored test on macOS?**
  **Resolved (deferred)**. With the test landed, two consecutive
  `just crap-ci` runs passed without any baseline edits — the gate
  is currently green because `hybrid_search.rs` resolves the
  cached model and exercises the embed path, providing incidental
  coverage. Neither candidate wiring is earned right now: Option A
  (`--include-ignored`) would also pull in
  `kenn-cli/tests/index_writes_analysis.rs`, which is a different
  ignored test with its own opt-in rationale; scoping `--include-ignored`
  to a single test target is more recipe complexity than the
  current pass-rate justifies. Option B (grandfather entries) would
  baseline functions that are not currently flagged, the exact
  "blindly baseline to silence" antipattern per CLAUDE.md. The
  `embed-smoke` recipe is the deterministic-on-demand check; if the
  gate fails on these functions again, revisit then with a real
  signal.

- **Should kenn-dotnet builds run this test?** No — `kenn-dotnet`
  is .NET driver code; the test belongs to `kenn-embed`. Out of
  scope.
