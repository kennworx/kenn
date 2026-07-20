## ADDED Requirements

### Requirement: each producer implementation has a deterministic integration test against real weights

Each in-process implementation of the embedding-producer boundary SHALL be exercised by a dedicated integration test that loads its real model weights and runs the produce-vectors path end-to-end, asserting on structural properties of the output (vector count matches input count, vector dimension matches the producer's reported `dim()`, vectors are L2-normalized, vectors are not all-zero, distinct inputs produce distinct vectors). The integration test SHALL run deterministically when invoked — its execution MUST NOT depend on environment variables that silently skip the embed path, and its coverage MUST attribute to the producer crate directly rather than through transitive calls from unrelated test suites. The integration test MAY be opt-in (gated by `#[ignore]` or a build feature) and MAY be platform-gated to match the implementation's platform support; it SHALL be runnable via a documented developer-facing command (e.g. a justfile recipe) so anyone touching the implementation can verify the contract without consulting other docs.

#### Scenario: the in-process llama producer is exercised against real EmbeddingGemma weights on macOS

- **WHEN** a developer runs the documented kenn-embed integration test recipe on macOS with the EmbeddingGemma model available (cached or downloaded)
- **THEN** `LlamaEmbedder::load()` resolves the model and initializes the llama backend
- **AND** `LlamaEmbedder::embed(...)` returns one L2-normalized vector per input string
- **AND** every returned vector has the dimension reported by `LlamaEmbedder::dim()`
- **AND** vectors produced for distinct input strings are themselves distinct
- **AND** the test fails loudly (does not silently skip) when the model is unavailable

#### Scenario: producer integration coverage does not depend on indirect test paths

- **GIVEN** the embed-producer integration test exists
- **WHEN** the test runs under coverage instrumentation
- **THEN** coverage of the producer's `embed` and backend-init functions is attributed directly to the producer crate
- **AND** the producer's crap-gate status does not depend on whether a downstream test suite (e.g. `kenn-store::hybrid_search`) happened to load the model
