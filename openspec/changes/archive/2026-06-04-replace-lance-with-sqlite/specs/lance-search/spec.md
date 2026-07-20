## MODIFIED Requirements

### Requirement: search store holds text, embeddings, and search indexes

The `db_default` backend SHALL persist, in the SQLite snapshot database, one row per
searchable unit (symbol-name entries and doc/comment entries). Each row SHALL carry its
source text, an optional embedding vector, and an `xxh3-64` content fingerprint of the row's
`embeddable_text`. The store SHALL maintain SQLite **FTS5** full-text indexes over the text
— a `trigram`-tokenized index for identifier (n-gram) search and a stemming-tokenized index
for prose/doc search. Vector search SHALL be served by a `sqlite-vec` `vec0` virtual table
performing **exact** brute-force nearest-neighbour search (no approximate ANN index); the
embedding vectors SHALL be reconciled from the committed sidecar. Because this is exact, it
SHALL return at least the quality of the prior Lance `IVF_PQ` approximate index — vector
results are not required to match the prior approximate ranking.

The `embeddable_text` SHALL be a single, fixed, documented string formula so the fingerprint
is reproducible across machines and tool versions.

#### Scenario: a row round-trips text, vector, and fingerprint

- **WHEN** a row is written with text, an embedding vector, and a fingerprint, then the store
  is reopened
- **THEN** reading the row back returns the same text, the same vector, and the same fingerprint

#### Scenario: full-text search is queryable without an embedding model

- **WHEN** the store is built and no embedding model is available
- **THEN** the FTS5 indexes are still constructed from the text
- **AND** full-text queries return ranked results
- **AND** the vector arm contributes no hits

### Requirement: identifier and BM25 ranking is deterministic and preserves prior ranking quality

Full-text queries SHALL return results ranked by FTS5 Okapi BM25, with kenn's identifier
ranking policy applied on top of the FTS5 candidates: an exact whole-name match SHALL be
boosted over n-gram matches, and the final order SHALL be `(score DESC, name length ASC, id
ASC)`. For a fixed corpus, tokenization, and query, the ranked result SHALL be deterministic.
The ranking SHALL preserve prior ranking quality — measured as top-k overlap against the
previous Lance-backed ranking on a fixed query set meeting the change's parity gate.

#### Scenario: equal-score results have a stable order

- **WHEN** two rows receive an identical score for a query
- **THEN** they appear in a fixed order determined by name length then id
- **AND** repeated runs of the same query produce the same ordering

#### Scenario: exact name match outranks an n-gram match

- **WHEN** a query exactly equals one symbol's name and is also a substring of others
- **THEN** the exact-match symbol ranks above the substring matches
