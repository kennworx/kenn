## Why

Both search stores are built for hybrid lexical + semantic retrieval, but
only the lexical half runs. `lance-search` already specs — and the code
already has — the `embedding` column, the native vector index builder, the
`xxh3-64` reconciliation that marks changed rows for re-embedding, and the
"committed embeddings survive clone and merge" guarantee. `findings-store`
likewise reserves an `embedding` column. What is missing everywhere is the
one component that turns text into vectors: an **embedding producer**.

Both `lance-search-backend` and `findings-backend` explicitly deferred it —
model selection was ruled out of scope at the time. The consequence is
visible across the codebase: the `embedding` column is always null,
`build_vector_index` short-circuits on zero vectors, `vector_search` /
`merge_hits` are dead code, `search_findings` is BM25-only, and
`store_finding` cannot detect near-duplicates. No semantic retrieval, no
paraphrase matching, no near-duplicate detection — for code or knowledge.

This change builds the producer and switches the dormant vector machinery
on. It also collects the two `findings-backend` tasks (2.2, 7.2) that were
deferred on exactly this dependency.

## What Changes

- Define a **pluggable embedding-producer boundary** — one interface that
  turns a batch of text into fixed-dimension vectors — so the model is
  swappable and storage/search depend on the boundary, not a model.
- Integrate a concrete embedding model behind that boundary — runtime
  resolved in design D2 (llama.cpp in-process via `llama-cpp-2`), backed by
  the runtime benchmark in `benchmark.md`.
- Produce embeddings **at index time** for code rows that `lance-search`
  reconciliation marks for re-embedding, and **at flush time** for findings.
- Activate the Lance **vector index** over the populated `embedding` column
  for both the code-search and findings datasets.
- Activate **hybrid search**: wire the vector kNN path into code
  `search_symbols_blended` and findings `search_findings` (both BM25-only
  today; the vector / merge code is present but dead).
- Complete the deferred `findings-backend` items: semantic near-duplicate
  detection in `store_finding` (was task 2.2) and the producer-boundary
  reuse for finding embeddings (was task 7.2).

## Capabilities

### New Capabilities

- `embedding-producer`: the pluggable producer boundary, the model integration behind it, embedding generation at index time and at findings-flush time, the index-time-only (offline query) guarantee, and activation of the vector index + hybrid search.

### Modified Capabilities

- `findings-store`: `search_findings` becomes hybrid lexical + vector, and `store_finding` surfaces semantically near-duplicate findings — the two requirements `findings-backend` shipped marked "deferred".

`lance-search` is **not** modified: its embedding, vector-index, and
"working vector search after clone" requirements already describe the end
state — this change fulfils them rather than changing them.

## Impact

- **New dependency:** `llama-cpp-2` — in-process llama.cpp, compiled from
  source as a normal cargo dependency, no server or daemon (design D2).
- **On-disk:** the `embedding` column gets populated; the vector index
  becomes live. `EMBEDDING_DIM` is pinned to 768 (EmbeddingGemma-300M,
  design D4) — set now, while no embeddings are committed.
- **Code:** `vector_search` / `merge_hits` (currently dead code) become
  reachable; `search_symbols_blended` and `search_findings` gain the vector
  arm; the index pipeline and the findings flush gain an embedding step.
- **Offline guarantee preserved:** embedding production happens only at
  index / flush time, never on the query path — committed embeddings stay
  readable after clone/merge with no model, per `lance-search`.
</content>
