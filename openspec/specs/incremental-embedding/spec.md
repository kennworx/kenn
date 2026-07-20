# incremental-embedding Specification

## Purpose
TBD - created by archiving change incremental-embedding. Update Purpose after archive.
## Requirements
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

The job SHALL submit its misses to the embedding producer as **bulk
(low) priority** so an interactive query embed is always served ahead of
it (see `embedding-producer`). The producer batches at the model's unit and
yields to interactive work between batches, so a large background pass
cannot monopolize the embedder or starve interactive search —
responsiveness does not depend on how the job frames its input.

The job SHALL consume the Lance scan as a stream and embed
**one scan batch at a time**. Texts and vectors SHALL NOT be
accumulated for the whole corpus before submission; each scan
batch's name-row texts are embedded, applied, and appended to the
build store before the next scan batch is pulled. Peak memory
SHALL be bounded by one scan batch plus one in-flight producer
request, independent of corpus size. A single doc-lookup pre-pass
over the scan is permitted (only doc strings are retained, not
the full record batches) where the schema requires cross-batch
lookup to compose the name+doc embed text.

The full re-embedding pass (the `kenn update` flow that fills a
freshly-built knowledge store with null embeddings) SHALL follow
the same per-scan-batch streaming discipline.

"Streaming" governs **stream consumption only** — the sidecar segment is
still **appended and hot-swapped atomically** (accumulate into a segment,
then publish by atomic rename), NOT published in torn partial pieces; a
crash mid-pass SHALL NOT leave a partial segment in the live set. Vectors
SHALL be applied in submission order so the published segment is independent
of batching.

#### Scenario: only the diff is embedded

- **WHEN** the background job runs after an index whose reconciliation left `M` misses
- **THEN** exactly `M` symbols are sent to the model and a new segment containing `M` entries is appended to `.kenn/vectors/`

#### Scenario: the background pass is low priority

- **WHEN** the background job submits its misses to the producer
- **THEN** they are classed bulk/low priority
- **AND** an interactive query embed issued concurrently is served ahead of the remaining bulk work

#### Scenario: scan is consumed as a stream, not collected

- **GIVEN** a knowledge store with `N` row groups in its Lance scan
- **WHEN** the embedding pass (full or incremental) runs
- **THEN** the pass holds at most one `RecordBatch` from the scan in memory at a time
- **AND** it does not call `try_collect()` on the scan stream
- **AND** each batch's vectors are applied and appended to the build store before the next batch is pulled

#### Scenario: the segment is published atomically despite streamed consumption

- **GIVEN** the job consumes the producer's vector stream incrementally to bound memory
- **WHEN** all misses are embedded
- **THEN** the segment is published by one atomic append + hot-swap, not as torn partial pieces
- **AND** a crash mid-pass leaves no partial segment in the live set

#### Scenario: a search stays responsive during a large background pass

- **GIVEN** a large background embedding pass in progress
- **WHEN** an interactive free-text search embeds its query
- **THEN** the query embed is served within roughly one model batch, not after the whole pass

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

Vectors SHALL be stored **per generation**, where a generation is
`(model_id, dim, quant, recipe)`. Reconciliation SHALL reuse a committed vector
only when its generation matches the active embedder's generation; the generation
is reflected in the sidecar layout (a per-generation namespace) so that multiple
generations coexist.

A generation change (new model id, dim, quant, or recipe) SHALL write vectors into
a **new** generation namespace and SHALL NOT wipe or invalidate prior generations —
there is no destructive whole-directory reset. Switching back to a prior generation
SHALL reuse its retained vectors with no re-embedding.

The model id SHALL be a plain string (e.g. `"embeddinggemma-300M"`); model
upgrades are expressed by versioning the id. Provider URL and content hashes are
not recorded.

#### Scenario: matching generation reuses vectors

- **GIVEN** a sidecar generation for `(embeddinggemma-300M, 768, int8, doc/v1)`
- **WHEN** the active embedder matches that generation
- **THEN** reconciliation reuses those vectors

#### Scenario: a generation change is additive, not destructive

- **GIVEN** an existing generation `(embeddinggemma-300M, 768, int8, doc/v1)`
- **WHEN** the recipe changes to `doc-gemma/v2`
- **THEN** the new generation is written into its own namespace
- **AND** the `doc/v1` vectors remain intact and reusable
- **AND** no whole-directory reset occurs

#### Scenario: switching back reuses the retained generation

- **GIVEN** both `doc/v1` and `doc-gemma/v2` generations exist
- **WHEN** the active embedder reverts to `doc/v1`
- **THEN** its vectors are reused with zero re-embedding

### Requirement: Committed versus derived store layout

Within the kenn store, `.kenn/vectors/` SHALL be the only committed artifact. The databases that store builds — the code-graph Lance datasets, the `knowledge/` Lance dataset, the BM25 indexes, and the IVF_PQ index — SHALL be classified as derived, gitignored, and rebuilt per worktree. There SHALL be no redb store. (The findings store, `.kenn/findings/`, is a separate durable Lance store on its own lifecycle — outside this requirement's scope; the `committed-findings` change governs its committed-versus-derived disposition.)

The derived Lance datasets — the code graph and the knowledge store — SHALL be co-located under `.kenn/local/` as one per-index-run snapshot: built into a single `building/` directory and published by one atomic directory swap. `.kenn/knowledge/` SHALL NOT remain a separate top-level path. `.kenn/.gitignore` therefore ignores `local/`, with `.kenn/vectors/` tracked as the committed embedding sidecar.

#### Scenario: a fresh worktree rebuilds derived state and reuses vectors

- **WHEN** a fresh git worktree or clone runs `kenn index`
- **THEN** the code-graph Lance datasets, the `knowledge/` Lance store, BM25, and IVF_PQ are rebuilt locally from source, and vectors are taken from the committed `.kenn/vectors/` sidecar — only that worktree's own diff is embedded

#### Scenario: derived datasets publish as one snapshot

- **WHEN** an index run finalizes
- **THEN** the code graph and the knowledge store are published together by a single atomic directory swap under `.kenn/local/`
- **AND** no derived Lance dataset remains at a separate top-level `.kenn/` path

### Requirement: Embeddable text is doc-only and skips undocumented symbols

The embeddable text for a symbol SHALL be its documentation prose only, not the
signature (the `sig\ndoc` blend is retired). A symbol with no documentation SHALL
NOT be embedded (no `vec0` row). The embeddable-text fingerprint that drives
incremental re-embedding SHALL be derived from the doc text only, so a
signature-only source change does not force a re-embed. Search SHALL function
correctly for symbols without a vector, using the lexical arms alone. The
doc-only recipe SHALL NOT regress documented-symbol conceptual recall versus
`sig+doc` on any measured corpus (validated: Rust +19% in-fusion, TypeScript
tie, C# +2% on cleaned docs).

#### Scenario: documented symbol embeds its doc only

- **GIVEN** a symbol with a documentation comment
- **WHEN** the embedding pass runs
- **THEN** the vector is computed from the doc prose, not `sig\ndoc`
- **AND** a signature-only edit to that symbol does not change its embeddable
  fingerprint

#### Scenario: undocumented symbol is not embedded

- **GIVEN** a symbol with no documentation
- **WHEN** the embedding pass runs
- **THEN** no `vec0` row is written for it
- **AND** it remains findable through the lexical (identifier / name-token /
  signature) search arms

### Requirement: The vector cache is garbage-collected

The vector store SHALL be garbage-collected so generations do not accumulate
unbounded (they span worktrees/projects when shared): it SHALL track
per-generation last-access time and evict least-recently-used generations past a
configurable size cap. GC SHALL be the only operation requiring a lock on the
vectors root (content-addressed appends remain lock-free), and it SHALL be
triggerable lazily (at index start) and explicitly (a `kenn gc` command).

#### Scenario: an idle generation is evicted under size pressure

- **GIVEN** the vector store exceeds its configured size cap
- **AND** a generation has not been accessed most recently
- **WHEN** garbage collection runs
- **THEN** that generation's vectors are evicted
- **AND** the active generation's vectors are retained

#### Scenario: appends do not block on GC

- **WHEN** a content-addressed vector append occurs concurrently with normal use
- **THEN** it proceeds without acquiring the GC lock

