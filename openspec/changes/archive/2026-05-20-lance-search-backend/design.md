## Context

The `db_default` storage backend composes two embedded engines: redb (the code graph — symbol/edge tables, B-tree traversal) and tantivy (two BM25 indexes — `symbols.name`, `symbol_docs.doc`). Both are throwaway: rebuilt deterministically per branch, never committed.

Two pressures break this arrangement:

1. **Semantic search is wanted.** Hybrid lexical + vector ranking needs embeddings. tantivy stores none.
2. **Embeddings are expensive and sometimes un-reproducible.** Producing them needs an embedding model — possibly an API, possibly unreachable in an offline dev environment. They must be *committed and shared*, not recomputed on clone.

A throwaway, per-branch store cannot satisfy "committed and shared." The code graph itself is fine as throwaway — it is cheap to rebuild and painful to merge (B-tree deltas across branches). But the embeddings need the opposite: a durable, git-committed, merge-clean home. That home also wants the BM25 index next to it, so hybrid queries hit one engine.

Lance is a columnar format with a native inverted (BM25) index and a native vector index. Prototyping (Python `pylance` exp1-5, Rust `lance` 6.0.0 spikes R1-R5) established that a Lance dataset can be committed to git, merged without conflicts, have its vector index preserved across merges, and produce BM25 scores identical to tantivy. This design adopts Lance on those verified properties and retires tantivy.

## Goals / Non-Goals

**Goals:**
- One committed, git-merge-clean Lance store holding symbol/doc text, embeddings, a BM25 index, and a vector index.
- Retire tantivy; the BM25 indexes move to Lance with no ranking regression.
- Embeddings survive `git clone` and `git merge` without recompute.
- Search and vector indexes survive a merge without a full rebuild — only the merged-in delta is indexed.
- Reconciliation on rebuild: unchanged symbols reuse committed embeddings; only changed/new ones are re-embedded.

**Non-Goals:**
- Embedding *generation* — model choice, local vs API, batching. Lance receives embeddings through a pluggable boundary; the producer is a follow-up.
- The findings/knowledge store and the MCP layer — separate proposals.
- Moving the code graph off redb. redb stays; it is the right tool for throwaway B-tree traversal.
- A git merge driver. The design's goal is that merges need none.

## Decisions

### D1 — Lance as the durable search store; redb stays for the code graph

Lance gives columnar storage + native BM25 + native vector search in one engine with a git-friendly file layout. Alternatives weighed: `sqlite-vec` (C dep, weaker vector story), a hand-rolled Arrow + `usearch` stack (more code, no FTS), SurrealDB-embedded (RocksDB/LSM, heavier, no committed-file model). Lance won on: native hybrid search, an immutable-file on-disk format that merges cleanly, and a committed format that makes embeddings shareable.

redb is **not** replaced. The code graph is throwaway and traversal-heavy; redb's B-tree range scans back adjacency lists and location lookups. Lance is columnar and scan-oriented — the wrong shape for that workload. The two stores serve opposite lifecycles (throwaway vs durable) and both are kept.

### D2 — Commit the data; the manifest and indexes are managed, not copied

A Lance dataset on disk is: `data/<uuid>.lance` fragments, `_indices/<uuid>/` index segments, and a manifest. All fragment and index files are UUID-named and immutable. The manifest is the only versioned, collision-prone file.

- **Fragments and index segments are committed.** They are immutable and uniquely named, so `git merge` unions them with zero conflicts.
- **The manifest is written by a custom `CommitHandler`** to a committed, collision-free path (ULID-named), *not* the default `_versions/<N>.manifest`. This removes the sequential-version filename collision and eliminates any snapshot/copy step — Lance writes the manifest straight to its committed location. (Validated: Rust spike R4.)
- Only a small derived pointer / local cache is gitignored.

Alternative considered: gitignore `_versions/` and regenerate the manifest from data files on load. It works (validated exp1B) but still leaves a derive-on-load step and orphans the index. The custom `CommitHandler` is cleaner.

### D3 — Stable row IDs; merge-time fragment renumbering

`enable_stable_row_ids` is on — rows carry a stable `u64` id independent of fragment position, so row identity survives any fragment rewrite.

Fragment IDs themselves are Lance-sequential within a branch. The implementation originally intended *content-derived* fragment IDs (a hash of the data-file UUID) so a merge would never renumber — but Lance caps a fragment ID at `u32` (`Manifest` does a `u32::try_from` on the max id), so a 64-bit content-hash ID space is not usable and a 32-bit one is collision-prone. Instead, the merge handler **renumbers** the other branch's recovered fragments to `max(resolved-manifest ids) + 1…`, which is collision-free by construction. Renumbering is harmless because the merge re-indexes the recovered delta anyway (see D4) — the index is rebuilt over the delta regardless, so no `fragment_bitmap` needs to survive. (Validated: the `merge_via_git_unions_rows` integration test — two branches, plain `git merge`, conflict-free union.)

### D4 — Index preservation across merge

A merge brings in the other branch's fragments and index segments (all committed, all merge-clean). The store then: restores the primary manifest, `Operation::Append`s the recovered fragments, and runs `optimize_indices` — which indexes **only the delta**, not the whole corpus. The bulk index is never rebuilt. (Validated: R3 / exp5 — a 200-symbol delta indexed in ~25 ms against an 8000-symbol committed index.)

Fallback: if a merge leaves the index unresolvable, a full rebuild is correct and bounded (linear; ~3–4 s per 100k rows in the spike). The fast path is preserved; the slow path is a safety net, not the norm.

### D5 — BM25 via Lance's inverted index

Lance's inverted index is Okapi BM25 with the standard defaults. A prototype (R5) indexed the same corpus in tantivy 0.26 and Lance under matched tokenization and got **byte-identical scores** across all queries. BM25 ranking is therefore not a risk.

Tokenizer mapping:
- `symbol_docs.doc` (whitespace + lowercase + ASCII-fold + snowball stem) maps 1:1 onto Lance's tokenizer filters.
- `symbols.name` uses a camel/Pascal-splitting tokenizer (`kenn_name`). Lance has no camel-case splitter. This needs either a custom tokenizer or an n-gram index. See Open Questions.
- The dual-field exact-match boost (`name_text` + `name_keyword`) maps onto two columns + a `BoostQuery`.

Equal-score tie-breaking differs between engines; a stable secondary sort (by id) makes ordering deterministic.

### D6 — Writer serialization is the store's responsibility

Lance's built-in concurrency guard is optimistic: it relies on an atomic rename-collision on `_versions/<N>.manifest`. The custom `CommitHandler` (D2) gives every manifest a unique name, so that collision never fires and the guard is bypassed. The store must therefore serialize writers itself — a single-process mutex around the commit critical section, with an `flock` on a lock file as a backstop against a stray second process.

### D7 — Embedding storage and reconciliation

Each row stores `{ text, embedding, fingerprint }` together — text and embedding never drift because they are one atomic row. The committed store may hold text that is stale relative to a branch's current source; rebuild reconciles it.

- **Identity** (which symbol is this): `(language, pub_id)` — already modeled and indexed. Fallback `(path, name, kind)` for symbols without a `pub_id`.
- **Change detection** (did the embeddable text change): an **xxh3-64** fingerprint over the exact `embeddable_text`. 64-bit avoids the silent stale-embedding bug a 32-bit hash risks; xxhash is already a project dependency.
- **File-level fast path**: an xxh3-64 hash of file bytes — self-computed, no git library, one code path for git and non-git directories. (The git blob SHA was considered and rejected: it would need a git library or the fragile git CLI, and is unavailable in non-git directories; self-hashing is cheap and universal.)
- **Reconcile on rebuild**: for each rebuilt symbol, look up the prior row by identity; if the fingerprint matches, reuse the committed embedding; otherwise re-embed. Stored identities not seen this run are deleted symbols.

### D8 — Dependency feature stripping

`lance` is taken with `default-features = false`, dropping six cloud storage backends (aws/azure/gcp/oss/huggingface/tencent) and geo (`lance-geo`). `datafusion` and `lance-namespace` (→ reqwest) are non-optional dependencies of the `lance` umbrella crate and cannot be stripped. `protoc` is needed at build time (system install, or Lance's `protoc` feature to vendor it).

### D9 — `.kenn/` splits into committed and local

The Lance store must be git-committed, but `.kenn/` was historically gitignored in full (throwaway redb snapshots). `.kenn/` is therefore split:

- `.kenn/knowledge/` — **committed**: the durable, repo-wide Lance store.
- `.kenn/local/` — **gitignored**: per-branch throwaway — the redb code-graph snapshots (`snapshots/`, `live`, `building/`, `runs/`, `index.lock`).
- `.kenn/.gitignore` — written by `layout::Store::open`; ignores `local/` plus the Lance store's local-only `.write.lock` / `_transactions/`. The repo-root `.gitignore` no longer mentions `.kenn/`.

This restructures the `index-store-layout` capability — `layout.rs` repoints the snapshot paths under `local/` (callers reach them through `Store` methods, so the blast radius is small) and `atomic_flip_live` now derives the symlink base from the symlink's own directory.

### D10 — The knowledge store has a build lifecycle: temp build → finalize → publish

The knowledge store is durable and committed, but an index run *rebuilds* it. Mutating `.kenn/knowledge/` in place is unsafe — a run that dies mid-rebuild leaves the committed store half-written — and it exposes readers to intermediate state. So the knowledge store gets a build lifecycle mirroring the redb snapshot lifecycle (`building/` → `snapshots/<ts>/`):

- **`begin_run`** — create an empty Lance store in a temporary, gitignored location, `.kenn/local/knowledge-build/`. A stale `knowledge-build/` left by a previously-failed run is discarded first.
- **`write_batch`** — append this batch's name/doc rows to the temp store. The batch is already in memory from the producer; it is written and dropped — nothing accumulates to end-of-run, so memory stays bounded (the same reason the redb stage batches). A name row needs the symbol's `sig`, which lives on the doc record; since `write_batch` writes redb first, the Lance stage point-reads `SYMBOL_DOCS` from redb for it. Per-batch append produces one data file and one manifest per batch — messy, but contained in the temp dir.
- **`end_run` — finalize, then publish:**
  - *Finalize* (in the temp dir): `compact_files` merges the per-batch data fragments into well-sized files; the BM25 / n-gram indexes are built; then the temp store is swept down to its latest manifest — the pre-compaction data files and every superseded manifest are deleted, leaving the compacted fragments, the index segments, and exactly one manifest.
  - *Publish* — an atomic directory swap. The finalized temp dir is itself a complete, valid Lance dataset, so publishing is two renames: the current `.kenn/knowledge/` is moved aside into a gitignored swap-out path, `knowledge-build/` is renamed onto `.kenn/knowledge/`, and the swapped-out dir is deleted. Both paths are under `.kenn/`, on one filesystem, so each rename is atomic — this mirrors the redb `building/` → `snapshots/<ts>/` flip. A whole-store swap, rather than relocating files into the existing store, is what keeps publish simple: the temp store is self-contained, its manifest version numbering restarts each run, and nothing in the published store is mutated in place. (A relocation scheme founders on the manifest — `CommitHandler::resolve_latest_location` resolves the latest by version number, and the temp store's version restarts low each run, so a relocated manifest would lose to the previous run's higher-versioned one.)

Crash safety: the swap is the only mutation of `.kenn/knowledge/`; until it runs, the published store holds the previous run's data, fully valid. A failed run is recovered by re-running — the next `begin_run` discards the stale temp dir. Readers therefore only ever observe a finalized store.

Relationship to reconciliation (D7): once the embedding producer exists, the temp build reads embeddings from the *currently published* `.kenn/knowledge/` and reuses them by fingerprint while ingesting batches, so republishing never loses committed embeddings. Until then the temp build is a plain rebuild.

Alternative considered — mutate `.kenn/knowledge/` in place via one `Operation::Overwrite` at end-of-run: simpler, but it either holds the whole graph in memory or re-scans redb, and gives no crash isolation. The temp-build lifecycle bounds memory, isolates failure, and keeps per-batch fragmentation out of the committed store.

## Risks / Trade-offs

- **Heavy dependency tree** (datafusion, reqwest via lance-namespace) → cannot be stripped via features. Mitigation: accept for now; a later option is to drop the `lance` umbrella crate and use `lance-file`/`lance-table`/`lance-index` directly, at the cost of the `Dataset`/`Scanner` API. Not done here.
- **Published-store file accumulation across runs — RESOLVED.** The directory-swap publish (D10) deletes the entire previous `.kenn/knowledge/` on every run, so no data / index / manifest files accumulate across runs. The finalize sweep keeps each run's store to its compacted fragments plus one manifest, and the swap discards everything older — no periodic GC pass is needed.
- **`protoc` build dependency** → Mitigation: document the requirement; optionally enable Lance's `protoc` feature so it is vendored and no system install is needed.
- **Camel-case tokenizer gap** for symbol-name search → Mitigation: a custom tokenizer, or an n-gram index (which also upgrades the current O(n) substring tier to an indexed lookup). Resolved in Open Questions before the symbol-search tasks.
- **`FileFragment::create_from_file` is marked "internal API" in Lance** → Mitigation: pin the `lance` version; the regeneration path also has a public-API route (`Operation::Overwrite` with distributed-write fragment collection).
- **Equal-score BM25 tie-break is engine-dependent** → Mitigation: stable secondary sort by id.
- **Snapshot format break** → old `db_default` snapshots are not forward-compatible. Mitigation: a layout-version check that triggers a rebuild with a clear message, never a silent misread.
- **Index-preservation path fails on an unforeseen merge shape** → Mitigation: full rebuild fallback (D4) — correct, bounded, just slower.

## Migration Plan

- Bump the `db_default` on-disk layout version. On opening an older snapshot, the version check reports "search store outdated — rebuilding" and rebuilds, rather than misreading.
- The change is gated by the existing `db_default` feature; that feature's dependency set swaps `tantivy` out for `lance`. No parallel backend or dual-write period — tantivy is removed in the same change.
- Rollback: revert the change. Snapshots written under the new layout are not readable by the reverted code; a rebuild restores them. Because the code graph (redb) and the search store (Lance) are both rebuildable from source, rollback loses no irreproducible data *except committed embeddings* — which, if the embedding producer is wired by then, would need recompute. While embedding generation is out of scope (this change), rollback is lossless.

## Resolved Decisions (were Open Questions)

- **`embeddable_text` definition — RESOLVED.** The Lance store holds two row kinds (mirroring the retired `symbols.name` / `symbol_docs.doc` indexes). `sig` (on the sparse `SymbolDocsRecord`) is a full signature string that already includes the symbol name (e.g. `fn bar()`, `class Foo`), so it is not concatenated with `name`. Per row kind:
  - **name row** `text` = `sig` when the symbol has docs, else `{kind} {name}`.
  - **doc row** `text` = `doc` (doc-only — preserves BM25 parity with the retired `symbol_docs.doc` index; no ranking regression).
  A name row stores its text in the `name_text` column, a doc row in `doc_text` (each null on the other row kind). The split is required because the two need different inverted-index tokenizers — an n-gram index for identifier search vs. a stemmed word index for prose — and one column carries one index. Whichever column the row populates is what BM25 indexes, what the embedding is produced from, and what the `xxh3-64` fingerprint covers.
- **Camel-case symbol tokenizer — RESOLVED: n-gram index.** Symbol-name search uses a character n-gram index, not a custom camel/Pascal tokenizer. The same n-gram index also upgrades the current O(n) full-table-scan substring tier to an indexed lookup.
- **Vector path population — RESOLVED: ship schema + query, empty.** This change wires the embedding column, the vector index, and the hybrid query path. Embeddings stay empty until the embedding-producer follow-up; no second schema migration. BM25 is fully live; the vector path is tested with fixture vectors.
- **Exact / prefix symbol-name tiers — RESOLVED: prototype before deciding.** Exact and prefix lookups are rare in search, so an n-gram-backed lookup may be acceptable, but it must be measured against the redb B-tree first. Until the prototype (task 7.3a) shows otherwise, exact/prefix stay on the redb B-tree; only the fuzzy and substring tiers move to Lance.
