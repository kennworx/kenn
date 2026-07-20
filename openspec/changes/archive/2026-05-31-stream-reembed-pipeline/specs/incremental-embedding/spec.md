## MODIFIED Requirements

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
