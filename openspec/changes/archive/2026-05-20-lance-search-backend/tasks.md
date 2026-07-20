## 1. Dependencies and scaffolding

- [x] 1.1 Add `lance`, `lance-index`, `lance-table`, `arrow-array`, `arrow-schema` to `crates/kenn-store/Cargo.toml` with `default-features = false`; gate them on the `db_default` feature.
- [x] 1.2 Remove `tantivy` from the `db_default` feature's dependency set. Done atomically with the search-path swap in task 7.4.
- [x] 1.3 Confirm a clean `cargo build`; enabled Lance's `protoc` feature to vendor protoc (no system install needed on any platform). Verified clean on macOS; Linux/Windows not buildable in this environment but the only remaining variable is the OS toolchain.
- [x] 1.4 Create the `crates/kenn-store/src/backends/db_default/lance/` module skeleton (`mod`, `store`, `schema`, `index`, `commit_handler`, `reconcile`).

## 2. Pin the embeddable-text contract

- [x] 2.1 Define and document the exact `embeddable_text` formula for symbol-name rows and for doc/comment rows (resolves the design Open Question).
- [x] 2.2 Implement the `xxh3-64` fingerprint over `embeddable_text` and over file bytes; unit-test reproducibility across runs.

## 3. Lance store core

- [x] 3.1 Define the Lance dataset schema: `id`, identity columns (`language`, `pub_id`; `path`, `name`, `kind` fallback), `text`, `embedding` (nullable fixed-size-list f32), `fingerprint`, and a row-kind discriminant (name vs doc).
- [x] 3.2 Implement dataset open/create with `enable_stable_row_ids`. Fragment IDs are Lance-sequential within a branch; merge-recovered fragments are renumbered above the resolved manifest's max id (task 5.2) — collision-free, and content-hash IDs are unusable since Lance caps fragment IDs at u32.
- [x] 3.3 Implement the custom `CommitHandler` writing manifests to a committed collision-free path (`manifests/{version}-{uuid}.manifest`); override `commit`, `resolve_latest_location`, and `resolve_version_location`.
- [x] 3.4 Implement single-writer serialization — an in-process mutex around the commit critical section plus a filesystem lock as a stray-process backstop.

## 4. Indexes and query

- [x] 4.1 Build the Lance inverted (BM25) index; configure the tokenizer to match the retired `kenn_doc` pipeline (simple + lowercase + stem + ASCII-fold).
- [x] 4.2 Implement the symbol-name n-gram index (resolved: n-gram, not a custom camel/Pascal splitter); exact-match boost via the `name` raw-keyword index + a boolean/boost query.
- [x] 4.3 Build the Lance vector index (IVF_PQ) over the `embedding` column — guarded: a no-op while embeddings are absent (an IVF_PQ index cannot train on zero vectors), wired for the producer follow-up.
- [x] 4.4 Implement the hybrid query path (BM25 name + doc, vector merge via `merge_hits`) with a stable tie-break by row id.

## 5. Merge and index preservation

- [x] 5.1 Committed layout is immutable + uniquely named (uuid data files, uuid-suffixed manifests); `write_gitignore` excludes only the local-only `.write.lock` and `_transactions/`.
- [x] 5.2 Implement merge handling (`reconcile_after_merge` → `append_recovered_fragments`): detect orphaned fragments, `Operation::Append` the recovered fragments, run `optimize_indices` over the delta only.
- [x] 5.3 Implement the full-rebuild fallback (`rebuild_from_data_files`): `Operation::Overwrite` all data files + rebuild indexes.
- [x] 5.4 Test (`merge_via_git_unions_rows`): two branches add disjoint rows → plain `git merge` → no conflict, store reconciles to the union.

## 6. Reconciliation on rebuild

- [x] 6.1 Implement identity resolution `(language, pub_id)` with `(path, name, kind)` fallback (`Identity::resolve`).
- [x] 6.2 Implement reconcile-on-rebuild (`reconcile`): reuse on fingerprint match, mark for re-embedding on change/new, treat unmatched committed rows as deleted.
- [x] 6.3 Implement the file-level fast path (`file_unchanged` + the `None`-fingerprint branch in `reconcile`).
- [x] 6.4 Define the pluggable embedding-producer boundary (`EmbeddingProducer` trait — interface only, no concrete model).

## 7. Wire into the backend and retire tantivy

- [x] 7.1 Implement the `Reader` search methods (`find_symbol_tiered`, `search_symbols_by_name`, `search_symbols_blended`) over the Lance store; hydrate hits to `SymbolRow` via the redb `short_id` join.
- [x] 7.2 Implement the `Writer` path: `write_batch` writes redb only; `end_run` rebuilds the Lance store from the full code graph (`build_search_rows` → `replace_all` → `build_search_indexes`). **Superseded by §10** — the in-place end-of-run rebuild is replaced by the temp-build → finalize → publish lifecycle (design D10).
- [x] 7.3a Prototype: benchmark the Lance n-gram index vs. the redb B-tree for exact/prefix lookups. **Dropped** — the resolved default keeps exact/prefix on the redb B-tree; no benchmark is needed unless that default is revisited.
- [x] 7.3 Move the symbol-search fuzzy and substring tiers to the Lance n-gram index; exact/prefix stay on the redb B-tree (tiers 1–2 in `find_symbol_tiered`).
- [x] 7.4 Remove `tantivy_schema.rs`, `tokenizer.rs`, and every tantivy code path from `reader.rs` / `writer.rs` / `mod.rs`; drop the `tantivy` dependency.

## 8. Migration and versioning

- [x] 8.1 Bump the `db_default` on-disk layout version. **Dropped** — the project is at 0.0.0 with no released snapshots; old data is dropped and re-indexed, not version-checked. Revisit when there are real users.
- [x] 8.2 Version check on an older snapshot. **Dropped** — same reason as 8.1: no versioning machinery while prototyping.

## 9. Verification

- [x] 9.1 BM25 ranking test: stable and deterministic ordering on a fixture corpus (`bm25_ranking_is_stable_and_deterministic`).
- [x] 9.2 Reconciliation tests: fingerprint match (reuse), change (re-embed), and unmatched row (delete) — covered by the `reconcile` unit tests.
- [x] 9.3 Clone test: a copied snapshot is searchable offline with no embedding model (`cloned_snapshot_is_searchable_offline`).
- [x] 9.4 `cargo clippy --workspace --all-targets` is zero-warning; the cross-backend fixture harness (`storage_fixtures`) passes.

## 10. Knowledge-store build lifecycle (design D10 — revises §7.2)

- [x] 10.1 `begin_run`: create an empty temp Lance store at `.kenn/local/knowledge-build/`; discard a stale one left by a failed run.
- [x] 10.2 `write_batch`: append the batch's name/doc rows to the temp store (per-batch append); resolve each name row's `sig` via a redb `SYMBOL_DOCS` point-read. Reuse one bridge runtime across the run rather than building one per batch.
- [x] 10.3 `end_run` finalize (in temp): `compact_files` to merge per-batch fragments; build the BM25 / n-gram indexes; sweep the temp store down to its latest manifest (drop pre-compaction data files + superseded manifests).
- [x] 10.4 `end_run` publish: an atomic directory swap — move the current `.kenn/knowledge/` aside into a gitignored swap-out path, rename `knowledge-build/` into place, delete the swapped-out dir. (Revises the design: a directory swap, not file relocation — see D10.)
- [x] 10.5 Crash safety test: a run that fails before publish leaves the previously-published `.kenn/knowledge/` intact and queryable.
- [x] 10.6 Manifest / data-file GC policy — eliminated by the directory-swap publish (10.4): each run discards the entire previous published store, so nothing accumulates across runs; no GC policy is needed.
