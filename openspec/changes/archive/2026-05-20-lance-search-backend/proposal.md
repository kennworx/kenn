## Why

The default storage backend pairs redb (code graph) with tantivy (BM25 text search). tantivy ranks full text but does nothing else — no vector search, and its indexes are throwaway, rebuilt per branch. The project needs hybrid lexical + semantic search, and the semantic half depends on embeddings that are expensive to produce (embedding-model calls, sometimes over a network unavailable in offline dev environments). Embeddings must therefore be a *shared, durable* asset — committed to git and merged across branches — not recomputed on every clone. tantivy can neither store nor serve vectors, and bolting a second store alongside it would fragment the search path. Adopting Lance as one columnar store for both BM25 and vector data — git-committed and merge-clean — yields a single hybrid-search engine and turns the costly embeddings into a shareable artifact.

## What Changes

- Add Lance (`lance` crate, `default-features = false`) as a storage component: columnar data + native inverted (BM25) index + native vector (IVF_PQ / HNSW) index.
- **BREAKING (`db_default` backend):** retire tantivy. The two BM25 indexes (`symbols.name`, `symbol_docs.doc`) move to Lance's inverted index. The `db_default` feature drops `tantivy`; adds `lance` and its support crates.
- The Lance store is **git-committed and merge-clean**: every on-disk file is UUID/ULID-named and immutable, so concurrent branches merge by file union with no conflicts and no merge driver.
- A custom Lance `CommitHandler` writes manifests directly to a committed, collision-free path — eliminating the `_versions/` directory and any copy step.
- Vector and BM25 indexes are **preserved across git merges** (no full rebuild): incremental `optimize_indices` covers only the merged-in delta.
- Embeddings are stored in the same Lance row as their source text; an **xxh3-64 content fingerprint** per row drives **reconciliation on rebuild** — symbols whose embeddable text is unchanged reuse their committed embedding, only changed/new symbols are re-embedded.
- Hybrid query: BM25 and vector search are served from one store and combinable (e.g. candidate-relative BM25 re-ranking over vector hits).
- Symbol-name search: the fuzzy BM25 tier moves to Lance; the substring fallback (current tier 3) becomes an n-gram-indexed lookup rather than a full table scan.
- **Out of scope:** embedding *generation* (model choice, local vs API) — Lance receives embeddings through a pluggable boundary; the producer is chosen and wired in a follow-up. The findings/knowledge store and the MCP layer are separate proposals.

## Capabilities

### New Capabilities
- `lance-search`: a Lance-backed search store — columnar storage of symbol/doc text plus embeddings, a native BM25 inverted index and a native vector index, a hybrid query surface, a git-committed merge-clean on-disk format (custom `CommitHandler`, UUID/ULID-named immutable files), index preservation across merge, and xxh3-64 fingerprint reconciliation on rebuild.

None.

The `Reader` trait's hybrid-search surface — `search_symbols_blended`, already specified by `storage-backend-abstraction` to accept text plus an optional vector — is unchanged. This change supplies the `db_default` backend *implementation* behind that surface; the trait contract does not move. The `db_default` feature's dependency set and on-disk layout change, but those are implementation details, not spec-level requirement changes. `index-store-db` specifies only redb tables (the code graph), which this change does not touch. The `compile-time backend selection` text in `storage-backend-abstraction` still names tantivy; it is already stale relative to the in-flight `diy-backend` change, and reconciling it is left to that change to avoid two concurrent edits to the same requirement.

## Impact

- **Code:** `crates/kenn-store/src/backends/db_default` — remove `tantivy_schema.rs`, `tokenizer.rs`, and tantivy paths in `reader.rs` / `writer.rs` / `mod.rs`; add a Lance store module. `crates/kenn-store/Cargo.toml` — the `db_default` feature drops `tantivy`, adds `lance`, `lance-index`, `lance-table`, `arrow-*`. `Reader` / `Writer` surface in `src/api`.
- **Dependencies:** adds `lance` and support crates. `default-features = false` strips the six cloud backends and geo; `datafusion` and `lance-namespace` (→ reqwest) remain non-optional. Removes `tantivy`. Net: a heavier build, one search engine instead of two. Requires `protoc` at build time (or Lance's `protoc` feature to vendor it).
- **On-disk / snapshots:** a new committed Lance store directory; tantivy index directories removed; `db_default` layout version bumped. Snapshots from the prior version are not forward-compatible with the new search path — handled by a version check / rebuild, not silent.
- **Git:** the Lance store is committed; only the derived manifest pointer / local cache is gitignored. Merges need no custom driver — every tracked Lance file is immutable and uniquely named. Writers within one worktree must be serialized by the store (single-process mutex), since a custom `CommitHandler` bypasses Lance's rename-collision concurrency guard.
- **Search behavior:** BM25 ranking is preserved — verified equal to tantivy on matched tokenization; equal-score tie-breaking may differ unless a stable secondary sort (by id) is added.
- **Out of scope (follow-ups):** embedding generation; the findings/knowledge store; the MCP layer.
