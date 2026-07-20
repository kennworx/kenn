## 1. Producer boundary

- [x] 1.1 Define the `EmbeddingProducer` trait: a batch of text → a batch of fixed-dimension vectors, with the dimension exposed.
- [x] 1.2 Pin `EMBEDDING_DIM` to 768 (design D4); runtime is `llama-cpp-2`, model EmbeddingGemma-300M q8_0 — the official `ggml-org` GGUF (design D2 / `benchmark.md`).
- [x] 1.3 Implement a concrete producer behind the trait via `llama-cpp-2` on Metal; add the dependency to `kenn-store`.

## 2. Embeddings at index time (code rows)

- [x] 2.1 At index time, embed every code row `lance-search` reconciliation marked for re-embedding; reuse the committed embedding for unchanged rows.
- [x] 2.2 Populate the `embedding` column on the Lance code-search dataset.
- [x] 2.3 Test: a changed symbol is re-embedded; an unchanged symbol keeps its committed vector.

## 3. Embeddings at flush time (findings)

- [x] 3.1 Embed each pending finding's `text` on `flush`; populate the findings `embedding` column.
- [x] 3.2 Test: a flushed finding carries a non-null embedding.

## 4. Vector index activation

- [x] 4.1 Build the Lance vector index over `embedding` for the code-search dataset once embeddings exist.
- [x] 4.2 Build the vector index over the findings dataset.

## 5. Hybrid search

- [x] 5.1 Wire the vector kNN arm into code `search_symbols_blended` — blend BM25 + vector via `merge_hits`.
- [x] 5.2 Wire the vector arm into findings `search_findings` (currently BM25-only).
- [x] 5.3 Test: a paraphrase query with no shared terms retrieves the right code symbol and the right finding.

## 6. Near-duplicate detection (findings task 2.2)

- [x] 6.1 In `store_finding`, embed the new text and probe committed findings by vector similarity; return matches above a threshold without auto-merging.
- [x] 6.2 Test: a semantically close prior finding is surfaced by `store_finding`.

## 7. Offline guarantee & query-side embedder

- [x] 7.1 Corpus embedding generation (symbols, findings) happens only at index / flush time — never on the query path.
- [x] 7.2 Free-text queries embed the query string via a lazily-loaded query embedder (load on demand, release after an idle TTL — design D7); item-to-item search reuses the committed vector with no model.
- [x] 7.3 Test: with no embedding model present, lexical and item-to-item vector search return results; free-text vector search degrades to lexical-only.

## 8. Verification

- [x] 8.1 Run `cargo clippy --workspace --all-targets` to zero warnings.
- [x] 8.2 Full hybrid-search test pass for both code and findings.
</content>
