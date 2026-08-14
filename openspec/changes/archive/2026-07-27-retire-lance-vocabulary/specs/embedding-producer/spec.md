## MODIFIED Requirements

### Requirement: the vector index and hybrid search activate once embeddings exist

Once the `embedding` data is populated, vector search SHALL activate over it for both the code-search and findings stores, and search SHALL blend lexical and vector similarity into one ranked result.

The two stores reach it differently, and the difference is deliberate. Code search SHALL serve its vector arm from a `sqlite-vec` `vec0` virtual table in the search database, declared over the embedding dimension with a cosine distance metric. Findings SHALL serve theirs by scoring their live-record set against the content-addressed findings sidecar under `<vectors_root>`; the embeddings are derived, so they do NOT live in the findings records. Bounding that scan with an ANN index is a future refinement and SHALL NOT be required here.

#### Scenario: retrieval by meaning

- **WHEN** a query paraphrases an indexed symbol or finding without sharing exact terms
- **THEN** hybrid search returns that symbol or finding among the ranked results

#### Scenario: findings embeddings are derived, not stored in the records

- **WHEN** a finding record is written
- **THEN** it carries no embedding
- **AND** its vector is derived into the findings sidecar, keyed by the finding text's fingerprint
