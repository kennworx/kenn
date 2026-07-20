## ADDED Requirements

### Requirement: a pluggable embedding producer turns text into vectors

The system SHALL define an embedding-producer boundary — a single interface that turns a batch of text into fixed-dimension float vectors and exposes that dimension. All embedding generation SHALL go through this boundary, so the underlying model is swappable without changes to storage or search code.

#### Scenario: text is embedded through the boundary

- **WHEN** a batch of text is passed to the producer
- **THEN** it returns one fixed-dimension vector per input
- **AND** every vector has the dimension the boundary reports

### Requirement: corpus embeddings are produced only at index time and flush time

Embeddings for **stored content** — code symbols and findings — SHALL be generated only when the code-search index is built or when findings are flushed, never on the query path. Committed embeddings SHALL remain readable and searchable with no embedding model and no network present.

Embedding a free-text query string into a query vector is a distinct query-time operation, governed by the requirement below; it never generates or modifies a stored embedding.

#### Scenario: a fresh clone searches without a model

- **GIVEN** a repository whose committed stores contain embeddings
- **WHEN** it is cloned into an environment with no embedding model and no network, and a search is run
- **THEN** lexical search and item-to-item vector search (reusing committed vectors) return ranked results
- **AND** free-text vector search degrades to lexical-only rather than failing

### Requirement: free-text queries are embedded by a lazily-loaded query embedder

A free-text search query SHALL be turned into a query vector using the same embedding model as the corpus, so the query and stored vectors share one space. The query embedder SHALL be loaded on demand and released after an idle period — an idle search service SHALL hold no embedding model in memory.

#### Scenario: a free-text query loads the embedder on demand

- **WHEN** a free-text vector search is issued and no query embedder is resident
- **THEN** the embedder is loaded, the query string is embedded, and hybrid results are returned
- **AND** after an idle period with no further queries the embedder is released

#### Scenario: item-to-item search reuses a committed vector

- **WHEN** a "similar items" search uses an already-indexed item as its source
- **THEN** that item's committed embedding is reused as the query vector
- **AND** no embedding model is loaded

### Requirement: code rows and findings are embedded through the producer

On an index run, every code row that `lance-search` reconciliation marks for re-embedding SHALL be embedded via the producer and have its `embedding` column populated; unchanged rows SHALL reuse their committed embedding. On a findings flush, every newly committed finding SHALL be embedded via the producer.

#### Scenario: a changed symbol is re-embedded

- **WHEN** an index run reconciles a symbol whose `embeddable_text` fingerprint changed
- **THEN** that symbol is embedded by the producer and its `embedding` column is populated

#### Scenario: a flushed finding carries an embedding

- **WHEN** a pending finding is flushed to the committed store
- **THEN** its `embedding` column is populated by the producer

### Requirement: the vector index and hybrid search activate once embeddings exist

Once the `embedding` column is populated, the Lance native vector index SHALL be built over it for both the code-search and findings datasets, and search SHALL blend BM25 and vector similarity into one ranked result.

#### Scenario: retrieval by meaning

- **WHEN** a query paraphrases an indexed symbol or finding without sharing exact terms
- **THEN** hybrid search returns that symbol or finding among the ranked results
</content>
