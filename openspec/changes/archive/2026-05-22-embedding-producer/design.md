## Context

`lance-search` and `findings-store` already provide everything hybrid
search needs except the vector producer: the `embedding` column
(`FixedSizeList<Float32, EMBEDDING_DIM>`, `EMBEDDING_DIM` a provisional
768), `build_vector_index` (a no-op while the column is null), the
`xxh3-64` fingerprint reconciliation that marks changed rows for
re-embedding, and `vector_search` / `merge_hits` (written, `dead_code`).

The missing piece is one component — text → vectors — plus the wiring that
calls it at the right time and turns the dormant machinery on. The hard
sub-problem is model selection, deliberately deferred until now.

## Goals / Non-Goals

**Goals:**
- A pluggable producer boundary the rest of the system depends on.
- A concrete model behind it, producing embeddings at index / flush time.
- Live vector index and hybrid search for code and findings.
- Semantic near-duplicate detection for `store_finding`.

**Non-Goals:**
- Re-ranking models or cross-encoders — first-pass retrieval only.
- Per-query embedding caches — embeddings are an index-time asset.
- Fine-tuning or training a model.

## Decisions

### D1 — A pluggable `EmbeddingProducer` boundary

One trait: a batch of text in, a batch of fixed-dimension vectors out, with
the dimension exposed. Storage and search depend on the trait; the model is
an implementation detail. This is the boundary `findings-backend` task 7.2
expected to reuse — it is defined here.

### D2 — Runtime: llama.cpp in-process via `llama-cpp-2`

`lance-search` requires that a fresh clone with no model and no network can
still run vector search over committed embeddings. That constrains only the
*query* path — it never needs the model. *Indexing* does, so the runtime
choice is purely an index-time concern.

A benchmark of every viable runtime (`benchmark.md`) settles this: **llama.cpp
in-process, via the `llama-cpp-2` crate.** It is the fastest and leanest option
measured, on both CPU and GPU, runs in-process with no server or daemon, and
compiles llama.cpp from source as an ordinary cargo dependency. The pure-Rust
alternative (Candle) is competitive on speed but costs 3–4× the memory in fp32,
and its quantized path is Metal-only. Measuring a local embedding server over
HTTP was shown to understate the runtime ~3× — in-process is both faster and the
only honest measurement.

The deployment device is **Metal** (Apple GPU).

The model behind the runtime is **EmbeddingGemma-300M, q8_0 GGUF** — the official
`ggml-org` GGUF specifically (a third-party conversion is broken; see
`benchmark.md` finding 4). It is 768-dim (see D4), **multilingual** (100+
languages), 2048-token context, and was the fastest and leanest multilingual
model measured — 635 t/s, 794 MB on Metal — validated to cosine 0.999 against the
ONNX reference. nomic-embed-text-v1.5 q8_0 remains the faster English-only
alternative behind the same runtime if multilingual retrieval is ever dropped.

### D3 — Embeddings are produced at index time and at flush time, never at query time

Code rows: the index run already reconciles rebuilt symbols and marks the
changed/new ones for re-embedding — embed exactly those, reuse the rest.
Findings: embed on flush, when pending findings are committed. The query
path never calls the producer.

### D4 — `EMBEDDING_DIM` is pinned to 768

The chosen model, EmbeddingGemma-300M, is 768-dimensional — so the provisional
768 column dimension becomes the committed one. A later model swap that keeps
768 dims needs no migration; one that changes the dimension (e.g. a move to
BGE-M3 or jina-v3, both 1024-dim — see `benchmark.md` Phase 2) does. Pin it now,
before the first embeddings commit.

### D5 — Activate the vector index and hybrid search

`build_vector_index` already builds once the column is non-empty. Hybrid
search reuses the existing `merge_hits` score-blend: run the BM25 arm and
the vector arm, merge. This lights up `search_symbols_blended` (code) and
`search_findings` (findings).

### D6 — Near-duplicate detection (findings task 2.2)

`store_finding` embeds the new text and runs a vector-similarity probe over
committed findings; matches above a threshold are returned to the caller.
No auto-merge — the decision stays with the caller, per the `findings-store`
design.

### D7 — Query-side embedder: lazy-loaded, Metal, same model

Free-text vector search must turn the query string into a vector — an
unavoidable query-time embedding. This refines D3: D3 forbids generating
*corpus* embeddings on the query path; embedding the *query string itself* is a
separate, far smaller operation. Two query shapes:

- **Item-to-item** ("findings like this one") reuses the source item's
  already-committed vector — no model, no network. This is the case the
  `lance-search` "fresh clone, no model" guarantee covers.
- **Free-text** queries need the embedder loaded.

Decision: the free-text query embedder is the **same EmbeddingGemma-300M q8
(ggml-org GGUF) on Metal**, **lazy-loaded with a short idle TTL** — loaded on
the first vector query, unloaded after idle. No second model or device to
maintain.

Measured (`benchmark.md`, Phase 3): a loaded query embedder costs ~558 MB peak
RSS on Metal; lazy-loading drops steady-state idle cost to ~0, paying that
558 MB only during an active query burst plus a ~1–3 s cold start on the first
query after idle. Two measurements ruled out the alternatives:

- **Metal beats CPU on memory** (558 vs 867 MB) — the llama.cpp CPU backend
  repacks quantized weights, ~300 MB of extra RAM. Use the same device as
  indexing.
- **q4 is rejected** — it saves only ~50 MB (EmbeddingGemma's token-embedding
  table stays high-precision) and the available q4 GGUF is broken (cosine
  −0.018, dropped dense head). Stay q8.

When no model is present, free-text vector search degrades to BM25-only;
item-to-item vector search still works from committed vectors. (The delta spec
requirement "embeddings are produced at index time, never at query time" is
scoped to *corpus* embeddings — query-string embedding is the D7 exception and
should be read as such.)

## Risks / Trade-offs

- **Model size / index-time latency.** A local model adds CPU/memory to
  every index run. → Batch embedding; embed only reconciliation-marked rows.
- **Dimension lock-in.** Once embeddings are committed, `EMBEDDING_DIM` is
  fixed without a migration. → Pin deliberately in D4 before the first run.
- **Embedding determinism across model versions.** A model upgrade changes
  vectors. → Treat a model-version change like a fingerprint change: mark
  for re-embedding. Out of scope to fully solve here; note it.

## Migration Plan

Additive. The `embedding` column already exists and is null everywhere;
this change starts populating it. No committed embeddings exist yet, so no
data migrates and `EMBEDDING_DIM` can still be set freely.

## Open Questions

- **Index-time budget.** Acceptable added wall-time per index run for
  embedding the changed-row delta.
- **Model-version tracking.** Where to record the producing model's identity
  (model + GGUF revision) so a version bump can trigger re-embedding.

## Resolved

- **Model selection.** Resolved to EmbeddingGemma-300M (D2): multilingual,
  768-dim, 2048 ctx, fastest multilingual model benchmarked. The earlier worry
  that EmbeddingGemma was "broken in GGUF" turned out to be a broken
  *third-party conversion* — the official `ggml-org` GGUF is correct (cosine
  0.999). Use that GGUF specifically. See `benchmark.md` Phase 2 and finding 4.
</content>
