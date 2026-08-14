## MODIFIED Requirements

### Requirement: Incremental background embedding job

The embedding pass SHALL run as a background job that embeds only the
reconciliation misses, appends the resulting sidecar segments, and hot-swaps the
new vectors into the searchable store. It SHALL be invokable both from the MCP
server's cold-start orchestration and from a CLI trigger.

The job SHALL submit its misses to the embedding producer as **bulk
(low) priority** so an interactive query embed is always served ahead of
it (see `embedding-producer`). The producer batches at the model's unit and
yields to interactive work between batches, so a large background pass
cannot monopolize the embedder or starve interactive search —
responsiveness does not depend on how the job frames its input.

The job SHALL consume its scan in **chunks** and embed one chunk at a time.
Texts and vectors SHALL NOT be accumulated for the whole corpus before
submission; each chunk's texts are embedded, applied, and appended to the
sidecar before the next chunk is scanned. Peak memory SHALL be bounded by one
chunk plus one in-flight producer request, **independent of corpus size**.

The chunk size SHALL be the configured embedding `batch_size` — the same value
the producer backends batch their own requests by — so the pass and the
producer cannot disagree about how much work is in flight.

The scan SHALL advance by a **rowid cursor**, not by offset and not by relying
on its own writes to shrink the candidate set: a full pass has no
"already embedded" filter to advance it, and an offset re-walks the skipped
prefix on every chunk. Rows with no embeddable text SHALL be excluded by the
scan query itself, so that an exhausted scan and a chunk of entirely-skipped
rows are not confusable.

The full re-embedding pass (the flow that fills a freshly-built knowledge store
with null embeddings) SHALL follow the same per-chunk discipline. It SHALL clear
the existing vectors in the **first** chunk's insert transaction, not before the
loop, so that an unavailable embedder — which is detected on the first
submission — never wipes vectors it cannot replace.

A full pass that fails partway SHALL leave the chunks it completed applied
rather than restoring the prior vectors. The resulting state is self-healing: a
subsequent incremental pass embeds exactly the rows still missing a vector.

"Chunking" governs **scan consumption and submission only** — each sidecar
segment is still written whole and published by atomic rename, never in torn
partial pieces; a crash mid-pass SHALL NOT leave a partial segment in the live
set. Vectors SHALL be applied in submission order.

#### Scenario: only the diff is embedded

- **WHEN** the background job runs after an index whose reconciliation left `M` misses
- **THEN** exactly `M` symbols are sent to the model and the resulting entries are appended to `.kenn/vectors/`

#### Scenario: the background pass is low priority

- **WHEN** the background job submits its misses to the producer
- **THEN** they are classed bulk/low priority
- **AND** an interactive query embed issued concurrently is served ahead of the remaining bulk work

#### Scenario: scan is consumed in chunks, not collected

- **GIVEN** a knowledge store with more embeddable rows than the configured `batch_size`
- **WHEN** the embedding pass (full or incremental) runs
- **THEN** the producer is called more than once
- **AND** no single call is larger than `batch_size`
- **AND** each chunk's vectors are applied and appended before the next chunk is scanned

#### Scenario: undocumented symbols do not stall the cursor

- **GIVEN** a corpus in which rows with no embeddable text are interleaved with documented ones
- **WHEN** the pass runs
- **THEN** every documented row is embedded exactly once
- **AND** the pass terminates rather than re-scanning a chunk it has already passed

#### Scenario: an unavailable embedder does not wipe existing vectors

- **GIVEN** a full re-embed pass and an embedder that is unavailable
- **WHEN** the pass runs
- **THEN** it reports the embedder as unavailable
- **AND** the previously published vectors remain intact

#### Scenario: the segment is published atomically despite chunked consumption

- **GIVEN** the job consumes the producer's vectors chunk by chunk to bound memory
- **WHEN** a chunk's vectors are appended
- **THEN** each segment is published by one atomic write + rename, not as torn partial pieces
- **AND** a crash mid-pass leaves no partial segment in the live set

#### Scenario: a search stays responsive during a large background pass

- **GIVEN** a large background embedding pass in progress
- **WHEN** an interactive free-text search embeds its query
- **THEN** the query embed is served within roughly one model batch, not after the whole pass
