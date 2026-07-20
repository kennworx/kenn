## Why

The default backend runs two storage engines: redb (the code graph —
symbols, edges, defs, aggregate, analysis) and Lance (the search /
knowledge store). The two were kept apart for one reason — opposite git
lifecycles: the code graph was throwaway, the knowledge store was
git-committed.

That reason is gone. The completed `incremental-embedding` change moved
the one expensive committed artifact into the `.kenn/vectors/` binary
sidecar and inverted `.gitignore`: `.kenn/knowledge/` (the Lance store)
is now **derived and gitignored**, exactly like `.kenn/local/` (the redb
snapshots). Both dual-engine databases are already throwaway, rebuilt
per worktree — one shared lifecycle, no split left to keep them apart.
(The findings store is a third, separately-governed Lance store that is
still committed; the `committed-findings` change brings it under the
same derived rule.)

So redb is now just a *second engine* — and a bespoke one. A `.lance`
dataset is Arrow-native, readable directly by DuckDB, Polars, pandas; a
`.redb` file is a B-tree only one Rust crate can open. With the
lifecycle split gone there is nothing left to justify it. A benchmark
spike (redb vs. Lance, 80k symbols / 839k edges) confirmed the move
carries no seconds-scale cost: the code-graph workload run the
Lance-native way — one bulk scan into an in-memory adjacency, traverse
in RAM — is ~4 ms, *faster* than redb's ~65 ms on-storage sweep.

This change makes the backend a **single engine, one derived store**.

## What Changes

- **BREAKING**: redb is removed — the `redb` and `bincode` dependencies,
  `db/key.rs` (composite-key codec), `db/schema.rs` (table definitions),
  `db/codec.rs`, and the redb reader/writer paths.
- The code graph moves to Lance datasets under `.kenn/local/`, alongside
  the knowledge store. Everything Lance, everything derived/gitignored;
  the committed `.kenn/vectors/` sidecar is untouched.
- One snapshot, one publish: all derived Lance datasets for a run are
  built in a single `building/` directory and published by **one atomic
  directory swap**. A reader only ever observes a fully-built snapshot.
- Read/write paths are reshaped to Lance-native access patterns —
  traversal/analysis bulk-scan into an in-memory CSR and traverse in RAM;
  hydration uses batched `take()`. **No per-item Lance query loops** —
  the one pattern that is seconds-slow, prohibited by design.
- redb's synchronicity is removed: `kenn-store` drops the
  `spawn_blocking` / `run_blocking` wrappers and the writer/reader
  surfaces become async-native. The ingester → DB-writer **channel and
  its OS thread are deleted** — each language ingester appends directly
  to the building datasets; Lance's optimistic concurrency resolves the
  concurrent commits (design D9).
- Key-collision dedup that redb did for free (e.g. `aggregate_edges`
  undirected dedup) moves into the writer — Lance is append-only and
  enforces no key uniqueness.
- **The custom commit / merge machinery is retired from the search
  store**: the bespoke `CommitHandler` (ULID manifest paths), the
  git-merge handler, fragment renumbering, and index-preservation-across-
  merge existed only to make Lance files survive `git merge`. The
  search / knowledge store is no longer committed, so that machinery is
  dead for it and removed. It stays live for the still-committed findings
  store; the separate `committed-findings` change retires it there.
- Scalar index usage: BTREE for search keys (`name_lower`, `pub_id`,
  `short_id`, `path`); BITMAP for low-cardinality filters
  (`kind`, `language`, `external`, `test`, edge kind). Edge data carries
  no scalar index — traversal bulk-scans it.

## Capabilities

### New Capabilities

<!-- none — this change re-engines existing capabilities -->

### Modified Capabilities

- `storage-backend-abstraction`: the backend is **one storage engine**
  (Lance), not "Lance + redb". The `write_batch` cross-engine
  non-atomicity caveat is dropped — there is one engine, and a run is
  published by a single atomic snapshot swap.
- `index-store-db`: the code-graph store moves from redb tables to Lance
  datasets. The `symbols`, `packages`, `defs`, `aggregate_nodes`,
  `aggregate_edges`, and `analysis_*` table layouts, the schema-break
  detection, and the analysis-write atomicity are restated in Lance
  terms — dropping every redb-ism (bincode values, big-endian composite
  keys, B-tree / unique indexes, feature-gated row deserialization).
- `indexing-orchestrator`: the ingester → DB-writer channel and the
  single DB-writer thread are removed — ingesters append directly to
  per-language Lance writers; finalize publishes one snapshot.
- `lance-search`: the custom `CommitHandler` and the git-commit /
  merge machinery are removed from the search store — it is no longer
  committed. This also clears spec debt from `incremental-embedding`,
  which made the Lance store derived in code but shipped without a
  `lance-search` delta.
- `mcp-symbol-search`: `find_symbol`'s `exact` / `prefix` tiers are
  resolved via a Lance scalar BTREE index, not the redb `SYMBOLS_BY_NAME`
  key — observable tier behavior is unchanged.
- `incremental-embedding`: the "Committed versus derived store layout"
  requirement drops the redb store and states that the code graph and
  knowledge store are co-located, derived Lance datasets under
  `.kenn/local/`, published as one per-index-run snapshot.

## Impact

- **Code**: `crates/kenn-store/src/db/` — `key.rs`, `schema.rs`,
  `codec.rs`, `writer.rs`, `reader.rs`, the `db/lance/store.rs` merge
  machinery (the `db/lance/commit_handler.rs` handler stays — the
  findings store still uses it), plus the redb-shaped call sites (per-hit
  `read_symbol`, per-vertex analysis range scans, `find_symbol_tiered`
  exact/prefix tiers, the `write_batch` `sig` point-read). `kenn-analyze`
  graph traversal switches to the in-memory CSR. The findings
  `CodeNodeResolver` — today `RedbCodeNodeResolver`, which probes the
  redb `SYMBOLS_BY_LANG_PUB_ID` table to flag stale findings — is
  re-implemented against the Lance code graph (the `findings-store`
  capability needs no spec change; its staleness requirement is
  engine-agnostic).
- **Dependencies**: `redb` and `bincode` dropped from `kenn-store`.
- **On-disk layout**: the knowledge store relocates from
  `.kenn/knowledge/` into the per-index-run snapshot under `.kenn/local/`,
  joining the code-graph datasets; `.kenn/knowledge/` ceases to exist as
  a top-level path. `layout.rs` repoints (`knowledge_dir()` folds into
  the snapshot path) and `.kenn/.gitignore` drops the now-dead
  `knowledge/` line, leaving `local/`. The `db_default` snapshot layout
  version is bumped; an older snapshot triggers a rebuild, never a
  misread.
- **Out of scope**: the `.kenn/vectors/` embedding sidecar
  (`incremental-embedding`, complete) is untouched; the `committed-findings`
  change is a parallel, consistent move but implemented separately; the
  legacy SurrealDB backend is unaffected; no Lance data becomes
  git-committed.
- Supersedes `lance-search-backend` design D1 ("redb stays") and retires
  its D2/D3/D4 commit-and-merge machinery.
