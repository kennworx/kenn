## ADDED Requirements

### Requirement: Lance store holds text, embeddings, and search indexes

The `db_default` backend SHALL persist, in a Lance dataset, one row per searchable unit (symbol name entries and doc/comment entries). Each row SHALL carry its source text, an optional embedding vector, and an `xxh3-64` content fingerprint of the row's `embeddable_text`. The store SHALL maintain a Lance native inverted (BM25) index over the text and a Lance native vector index over the embedding column.

The `embeddable_text` SHALL be a single, fixed, documented string formula so the fingerprint is reproducible across machines and tool versions.

#### Scenario: a row round-trips text, vector, and fingerprint

- **WHEN** a row is written with text, an embedding vector, and a fingerprint, then the store is reopened
- **THEN** reading the row back returns the same text, the same vector, and the same fingerprint

#### Scenario: BM25 index is queryable without an embedding model

- **WHEN** the store is built and no embedding model is available
- **THEN** the BM25 inverted index is still constructed from the text
- **AND** full-text queries return ranked results

### Requirement: BM25 ranking is deterministic and preserves prior ranking quality

Full-text queries SHALL return results ranked by Okapi BM25. For a fixed corpus, tokenization, and query, the ranked result SHALL be deterministic — ties on equal BM25 score SHALL be broken by a stable secondary sort on row id.

#### Scenario: equal-score results have a stable order

- **WHEN** two rows receive an identical BM25 score for a query
- **THEN** they appear in a fixed order determined by row id
- **AND** repeated runs of the same query produce the same ordering

### Requirement: the Lance store is git-committed and merges without a custom driver

Every Lance file the store persists — data fragments and index segments — SHALL be immutable after creation and named by a UUID or ULID. Merging two branches that each added rows SHALL union their files with no git conflict and SHALL NOT require a git merge driver.

#### Scenario: two branches add disjoint rows and merge cleanly

- **GIVEN** branch A and branch B each committed new rows to the store
- **WHEN** branch B is merged into branch A with a plain `git merge`
- **THEN** the merge completes with no conflict on any tracked Lance file
- **AND** the merged store contains every row from both branches

### Requirement: the manifest is written to a committed collision-free path

The store SHALL install a custom Lance `CommitHandler` that writes each manifest to a committed, uniquely-named path. The store SHALL NOT depend on Lance's default `_versions/<N>.manifest` location, and the sequential manifest filename SHALL NOT appear as a git-tracked file.

#### Scenario: a commit places the manifest at the custom path

- **WHEN** the store commits a write
- **THEN** the manifest appears at the store's custom committed path
- **AND** no `_versions/` directory is created as a tracked artifact
- **AND** reopening the store through the same `CommitHandler` reads the committed manifest

### Requirement: search indexes are preserved across a merge

After a git merge brings in another branch's fragments and index segments, the store SHALL extend its existing indexes to cover only the merged-in delta and SHALL NOT rebuild the full index. If the index cannot be extended, the store MAY fall back to a full rebuild, which SHALL produce a correct index.

#### Scenario: merging a delta indexes only the new rows

- **GIVEN** a store with a built index over N rows and a merged-in branch adding K rows
- **WHEN** the store is opened after the merge and its indexes are optimized
- **THEN** the pre-existing N-row index is reused, not rebuilt
- **AND** queries return results covering all N + K rows

### Requirement: committed embeddings survive clone and merge without recompute

An embedding stored in a committed row SHALL be readable after `git clone` and after `git merge` with no call to an embedding model. The expensive embedding artifact SHALL be a shared, durable asset, not regenerated per checkout.

#### Scenario: a fresh clone has working vector search

- **GIVEN** a repository whose committed store contains embeddings
- **WHEN** the repository is cloned into an environment with no embedding model and no network
- **THEN** vector search over the committed embeddings works without producing any embedding

### Requirement: reconciliation on rebuild reuses unchanged embeddings

The code graph is rebuilt from source per branch. On rebuild, the store SHALL reconcile rebuilt symbols against the committed store by identity `(language, pub_id)`, falling back to `(path, name, kind)` when no `pub_id` exists:

- A rebuilt symbol whose `embeddable_text` fingerprint matches the committed row SHALL reuse the committed embedding.
- A rebuilt symbol that is new, or whose fingerprint differs, SHALL be marked for re-embedding.
- A committed row with no corresponding rebuilt symbol SHALL be treated as a deleted symbol.

A file-level fast path MAY skip per-symbol fingerprinting when an `xxh3-64` hash of the file's bytes is unchanged since the last index.

#### Scenario: unchanged symbol keeps its embedding

- **GIVEN** a committed row for a symbol with fingerprint F
- **WHEN** the graph is rebuilt and the same symbol still hashes to F
- **THEN** the symbol is not marked for re-embedding and reuses the committed embedding

#### Scenario: changed symbol is marked for re-embedding

- **WHEN** a rebuilt symbol's `embeddable_text` hashes to a value different from its committed row
- **THEN** the symbol is marked for re-embedding

#### Scenario: removed symbol is reconciled as deleted

- **WHEN** a committed row's identity has no match among the rebuilt symbols
- **THEN** that row is treated as a deleted symbol

### Requirement: a single store is written by one writer at a time

Because the custom `CommitHandler` gives every manifest a unique name, Lance's rename-collision concurrency guard does not engage. The store SHALL serialize writers to one store itself — an in-process mutex around the commit critical section, with a filesystem lock as a backstop against a second process.

#### Scenario: concurrent writes do not fork the store

- **WHEN** two write operations against the same store are issued concurrently
- **THEN** they are serialized
- **AND** the resulting store has a single coherent latest manifest, not two divergent ones
