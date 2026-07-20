## MODIFIED Requirements

### Requirement: Model-identity manifest gating

`.kenn/vectors/manifest.toml` SHALL record the embedding model
identity (the model id string), the vector dimension, and the
quantization. Reconciliation SHALL reuse committed vectors only
when the active embedder's identity matches the manifest's
`embedding_model.id`.

The identity is a **plain model id string** (e.g.
`"embeddinggemma-300M"`). Provider URL and content hashes
(previously `gguf_xxh3`) are **not** recorded — provider URL is
a runtime concern that does not bind the vectors, and a content
hash cannot be obtained for vectors produced by remote
OpenAI-compatible providers (ollama, lm-studio, hosted APIs).

Model upgrades SHALL be expressed by versioning the id
(`embeddinggemma-300M-v1` → `embeddinggemma-300M-v2`), the
universal convention across the OpenAI / ollama / lm-studio
ecosystem. A bytes-changed-under-same-id swap is the operator's
foot-gun, not the manifest's to detect.

The manifest table is named `[embedding_model]` (renamed from
the previous `[model]`).

#### Scenario: matching id reuses vectors

- **GIVEN** a sidecar whose manifest records `embedding_model.id = "embeddinggemma-300M"`
- **WHEN** the active embedder reports `identity() == "embeddinggemma-300M"`
- **THEN** reconciliation reuses the committed vectors

#### Scenario: mismatched id triggers a full rebuild

- **GIVEN** a sidecar whose manifest records `embedding_model.id = "embeddinggemma-300M-v1"`
- **WHEN** the active embedder reports `identity() == "embeddinggemma-300M-v2"`
- **THEN** the sidecar is treated as fully missing and a full re-embed is required

#### Scenario: same id across different providers is treated as compatible

- **GIVEN** a sidecar whose manifest records `embedding_model.id = "nomic-embed-text"` produced via one OpenAI-compatible provider
- **WHEN** the active embedder is a different OpenAI-compatible provider reporting `identity() == "nomic-embed-text"`
- **THEN** reconciliation reuses the committed vectors
- **AND** any small numerical drift between providers for the same id is accepted as noise

#### Scenario: old [model] manifest is treated as incompatible

- **GIVEN** a sidecar whose manifest was written before this change and uses the `[model]` table with `gguf_xxh3`
- **WHEN** the active embedder reconciles against it
- **THEN** the manifest is treated as incompatible
- **AND** reconciliation proceeds as if the sidecar were fully missing
