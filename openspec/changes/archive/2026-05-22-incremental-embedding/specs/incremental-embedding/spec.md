## ADDED Requirements

### Requirement: Content-addressed vector sidecar

Embeddings SHALL be persisted in a committed sidecar at `.kenn/vectors/` as a
`fingerprint → vector` map, where `fingerprint` is the `xxh3-64` of the symbol's
`embeddable_text`. Vectors SHALL be stored int8-quantized at full model
dimension (per-vector symmetric scalar quantization).

#### Scenario: vector stored under its content fingerprint

- **WHEN** the embedding job produces a vector for a symbol with embeddable text `T`
- **THEN** the vector is written to the sidecar keyed by `xxh3-64(T)`, int8-quantized with a per-vector f32 scale, at the full model dimension

#### Scenario: identical text yields one entry

- **WHEN** two symbols share byte-identical `embeddable_text`
- **THEN** they resolve to the same fingerprint and the sidecar holds a single shared vector entry

### Requirement: Fingerprint reconciliation at index time

`kenn index` SHALL reconcile the structural store against the sidecar: a symbol
whose `embeddable_text` fingerprint is present in the sidecar SHALL have its
embedding filled from the sidecar without invoking the model; a symbol whose
fingerprint is absent SHALL be left unembedded and queued for the background job.

#### Scenario: cached fingerprint is reused

- **WHEN** `kenn index` ingests a symbol whose fingerprint is present in the sidecar
- **THEN** the symbol's `embedding` column is filled from the committed vector and the model is not invoked for it

#### Scenario: miss is queued

- **WHEN** `kenn index` ingests a symbol whose fingerprint is absent from the sidecar
- **THEN** the embedding is left null and the symbol is queued for the background embedding job

### Requirement: Incremental background embedding job

The embedding pass SHALL run as a background job that embeds only the
reconciliation misses, appends one sidecar segment, and hot-swaps the new
vectors into the searchable store. It SHALL be invokable both from the MCP
server's cold-start orchestration and from a CLI trigger.

#### Scenario: only the diff is embedded

- **WHEN** the background job runs after an index whose reconciliation left `M` misses
- **THEN** exactly `M` symbols are sent to the model and a new segment containing `M` entries is appended to `.kenn/vectors/`

#### Scenario: search is available before the job finishes

- **WHEN** the MCP server starts and the background embedding job has not yet completed
- **THEN** BM25 search is available immediately and vector search returns results for all already-cached vectors, without waiting for the job

### Requirement: Segment append-log and compaction

Each background-job run SHALL append a uniquely-named segment file. Compaction
SHALL fold all segments plus the baseline into a single baseline, retaining only
entries whose fingerprint is in the current live corpus and whose model matches
the manifest.

#### Scenario: concurrent branches do not conflict

- **WHEN** two branches each append a segment off a shared parent commit and are later merged
- **THEN** git merges both segment files cleanly, because each run writes a new uniquely-named file and never modifies an existing one

#### Scenario: compaction evicts dead and stale entries

- **WHEN** compaction runs
- **THEN** the rewritten baseline retains only entries whose fingerprint appears in the freshly-built structural store and whose model matches the manifest, and the superseded segment files are removed

#### Scenario: the log is usable without compaction

- **WHEN** a build reads a sidecar of multiple un-compacted segments
- **THEN** reconciliation unions all segments into one `fingerprint → vector` lookup and proceeds normally

### Requirement: Model-identity manifest gating

`.kenn/vectors/manifest.toml` SHALL record the embedding model identity, vector
dimension, and quantization. Reconciliation SHALL reuse committed vectors only
when the active embedder's identity matches the manifest.

#### Scenario: matching manifest reuses vectors

- **WHEN** the active embedder's identity matches the manifest
- **THEN** reconciliation reuses the committed vectors

#### Scenario: mismatched manifest defers to a full rebuild

- **WHEN** the active embedder's identity differs from the manifest
- **THEN** the sidecar is treated as fully missing and a synchronous full re-embed (`kenn update`) is required to regenerate it

### Requirement: Committed versus derived store layout

`.kenn/vectors/` SHALL be the only committed store artifact. The redb store, the
`knowledge/` Lance dataset, the BM25 indexes, and the IVF_PQ index SHALL be
classified as derived, gitignored, and rebuilt per worktree.

#### Scenario: a fresh worktree rebuilds derived state and reuses vectors

- **WHEN** a fresh git worktree or clone runs `kenn index`
- **THEN** redb, the `knowledge/` Lance store, BM25, and IVF_PQ are rebuilt locally from source, and vectors are taken from the committed `.kenn/vectors/` sidecar — only that worktree's own diff is embedded
