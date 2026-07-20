# lance-search

## Purpose

This spec defines the SQLite-backed search store for the `db_default`
backend: storage of symbol/doc text alongside embeddings, SQLite FTS5
full-text indexes (trigram for identifiers, stemming for prose) and a
`sqlite-vec` `vec0` exact brute-force KNN vector arm, embedding vectors
reconciled from the committed sidecar, and `xxh3-64` fingerprint
reconciliation on rebuild. It turns expensive embeddings into a shared,
durable asset and serves hybrid lexical + semantic search from a single
store. (The capability is still named `lance-search` for continuity;
renaming it to `code-search` is a separate deferred follow-up.)
## Requirements
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

### Requirement: Search dataset id columns follow the store naming convention

The search dataset's identity columns SHALL be named so each states its contract:
- `id` — the **volatile** numeric join key (was `short_id`), rewritten every run, resolved against the graph dataset matching the row's kind (symbol or file).
- `pub_id` — the symbol's **stable, API-visible** public id (unchanged), e.g. `cs:Foo`; empty for non-symbol rows. This is the same `pub_id` meaning used elsewhere in the store.
- `embed_key` — the **internal** composite key used to reconcile and reuse committed embeddings across runs (was `id` / `stable_id`): `name:<lang>:<pub_id>`, `doc:<lang>:<pub_id>`, or `filedoc:<lang>:<path>`. Not API-visible.

#### Scenario: Search row columns use the convention

- **WHEN** the search store is built
- **THEN** each row's volatile join key is `id`, the symbol's public id is `pub_id`, and the internal embedding-reconciliation key is `embed_key`

#### Scenario: embed_key drives reconciliation, volatile id drives join

- **GIVEN** a row whose `embed_key` and text fingerprint are unchanged since the last run
- **WHEN** the search store is rebuilt
- **THEN** its committed embedding is reused even though its `id` (volatile join key) may have changed

### Requirement: File docs are indexed as path-identified doc rows

The search-store build SHALL join each `file_docs` row to its `files` row (`file_id → files.id`) for path and language, and emit one `Doc`-kind search row feeding the same BM25 doc inverted index as symbol docs. The row SHALL set `pub_id` empty, `path` to the file's normalized workspace-relative path (the same value stored on the `files` row), `embed_key = "filedoc:<lang>:<path>"` (the `filedoc:` prefix keeps it disjoint from symbol-doc `embed_key`s `doc:<lang>:<pub_id>`), `doc_text` to the file's joined doc text, `row_kind = doc`, and `id` to the file's **real `id`** (the file dataset's join key). Because file and symbol ids are independent id spaces, the `id` on a file row is only meaningful against the `files` dataset — hydration MUST resolve it there, not against `SYMBOLS` (see mcp-symbol-search). No separate index or text-analysis path SHALL be introduced — file doc rows are ordinary doc rows distinguished by their empty `pub_id` / path identity.

#### Scenario: A file doc becomes a BM25-searchable doc row

- **GIVEN** a `file_docs` row for `src/OrderIntake.cs` with doc text `"Handles order intake validation."`
- **WHEN** the search store is built
- **THEN** the search dataset contains a `row_kind = doc` row with `pub_id` empty, `path = "src/OrderIntake.cs"`, `embed_key = "filedoc:csharp:src/OrderIntake.cs"`, and `doc_text = "Handles order intake validation."`
- **AND** a BM25 query for `order intake` matches that row

#### Scenario: File doc rows reconcile by fingerprint like symbol docs

- **WHEN** a rebuild runs and a file's doc text is unchanged
- **THEN** the row's `embed_key` and `xxh3-64` text fingerprint are unchanged and any committed embedding is reused, identically to symbol doc rows

