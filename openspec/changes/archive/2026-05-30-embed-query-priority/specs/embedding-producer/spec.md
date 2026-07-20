## ADDED Requirements

### Requirement: Embedding requests carry an interactive-vs-bulk priority

The embedding-producer boundary SHALL distinguish **interactive** embeds
(a free-text query being vectorized for search) from **bulk** embeds
(background corpus embedding). This intent SHALL be carried from the call
site through the producer to the inference processor — it is not enough to
collapse a query into an ordinary batch embed, because the processor needs
the class to schedule it.

Interactive query embeds SHALL be classed **high** priority; background
bulk embeds **low**. When no class is supplied, a request defaults to
**bulk** (the safe default for the corpus pass).

#### Scenario: a query embed and a bulk embed are distinguishable at the producer

- **WHEN** a free-text query is embedded and, separately, a background corpus batch is embedded
- **THEN** the producer sees the query as high priority and the bulk batch as low priority
- **AND** a caller that supplies no class is treated as bulk

### Requirement: The inference worker batches at the model unit and serves queries ahead of bulk

The producer's inference worker SHALL run at most one model encode in
flight (the serialized-inference invariant) AND SHALL guarantee that an
interactive query embed is served ahead of pending bulk work, bounding the
query's wait to at most **one in-flight model batch** — never a whole bulk
request or pass.

The worker SHALL process inference in units of **one encode over at most
the model's internal batch size** (`SEQS_PER_BATCH`), reusing a single
resident model/context across batches rather than per-request. Inputs of
either priority are taken in batches of that size; the worker SHALL serve
all ready **high**-priority (interactive) batches before the next **low**
(bulk) batch. A batch SHALL NOT exceed `SEQS_PER_BATCH`: a large request is
processed as a sequence of such batches, and packing small same-class
requests together fills a batch only up to that ceiling. Each request's
results SHALL be reassembled in input order before it returns. This applies
to the **in-process** producer; the daemon worker is governed by
`embeddings-api` and SHALL be the same shared component.

A large bulk request therefore cannot occupy the worker beyond a single
model batch at a time; between batches a newly-arrived query batch is taken
first.

#### Scenario: a query embed preempts a large bulk request at the next batch

- **GIVEN** a large bulk embed request whose batches are queued/encoding in-process
- **WHEN** an interactive query embed arrives
- **THEN** the query is encoded after at most the one model batch currently in flight, not after the whole bulk request
- **AND** the bulk request still receives every one of its vectors, in input order

#### Scenario: one in-flight encode is preserved, context reused

- **WHEN** the worker is encoding any batch
- **THEN** no second encode runs concurrently
- **AND** priority takes effect at batch boundaries, not by interrupting an in-flight encode
- **AND** the resident model/context is reused across batches, not recreated per request
