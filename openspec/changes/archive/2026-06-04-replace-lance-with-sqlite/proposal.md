## Why

Lance (with DataFusion + Arrow) is the heaviest thing in kenn's build, and a spike
(`tmp/sqlite-spike/REPORT.md`) showed SQLite can do every job kenn actually asks of it.

Measured on the shipping `kenn` CLI (release profile):

- **62 of 436 dependency crates (14%)** are lance / datafusion / arrow / parquet /
  sqlparser / object_store.
- **Clean release build: 14m 27s**, ~14 GB peak RSS. lance/df/arrow are **35%** of compile
  unit-seconds, and the serial critical-path tail is **`kenn-cli` at ~7 min** — a fat-LTO
  link that exists almost entirely to optimize the lance/df graph into the binary.
- **64 MB** release binary; lance/df/arrow ≈ 53% of compiled rlib mass.

The spike proved the search side survives the swap:

- **Vector search** — kenn searches the **f32** embedding column, so brute-force f32 is
  exact (100% parity). Brute-force int8 (the compact path) ran **1.8 ms @100k, 19 ms @1M**,
  scalar/single-thread — comfortably within budget at any single-repo scale.
- **Identifier search** — SQLite FTS5(trigram) was **~20× faster** than Lance's n-gram
  index (p95 269 µs vs 5336 µs) with 74.7% top-10 overlap; the gap is *ranking policy*
  (kenn's exact-match boost + length tiebreak), not retrieval.

kenn's storage is already hidden behind `api::Reader` / `open_reader` / `open_writer`, and
the trait's own docs anticipate a non-Lance backend ("the future `tantivy + redb + hnsw_rs`
backend"). This change is the realization of that: swap the engine, keep the surface.

## What Changes

- **Replace the Lance storage backend with SQLite**, entirely behind the existing
  `api::Reader` and the `DbReader` / `DbWriter` concrete types. No `kenn-mcp` or
  `kenn-indexer` call site changes.
  - **Code graph** (symbols / defs / files / edges / packages / aggregate / analysis):
    SQLite tables with the same column semantics; the open-time bulk scan that builds the
    in-memory CSR projection becomes a `SELECT`.
  - **Identifier search:** SQLite **FTS5 (`trigram`)**, with kenn's exact-match boost +
    name-length tiebreak replicated on top of the FTS5 candidate list to preserve ranking.
  - **Prose / doc search:** SQLite **FTS5 (`porter`/`unicode61`)** BM25, covering symbol-doc
    and `file_docs` (path-identified) rows.
  - **Vector search:** **`sqlite-vec`** `vec0` virtual tables — a single-file C extension
    with no Rust dependency tree, riding the SQLite we already bundle. The parity path stores
    `vec0 float[768]` and does exact, SIMD-accelerated brute-force; `int8`/`bit` `vec0` types
    are an optional compact/fast path (recall tradeoff), not the exact path. This is a
    **correctness upgrade**: it replaces Lance's *approximate* `IVF_PQ` ANN index with exact
    nearest-neighbour search, well within budget at kenn scale (sub-2 ms @100k). No ANN index
    is introduced; a hand-rolled brute-force loop is the zero-dep fallback.
- **Port the findings store too.** The findings store is a *second* Lance store with its own
  embeddings, inverted index, and vector sidecar (`.kenn/findings/vectors/`), serving the
  `search_findings` hybrid query. It moves to its own SQLite database (FTS5 + brute-force over
  the findings sidecar) — Lance cannot be dropped while findings still use it. The committed
  per-finding `<id>.json` records and the MCP tool contracts are unchanged.
- **Three independently-published SQLite databases**, mirroring today's independently-swapped
  Lance dirs: `graph.db`, `knowledge.db` (code search), `findings.db`. The background embed
  job republishes `knowledge.db` alone; findings stage/publish separately — a single
  monolithic file would break those independent swaps.
- **Drop `lance*`, `datafusion*`, `arrow*`, `parquet`, `sqlparser`, `object_store`** from the
  workspace; add `rusqlite` (bundled) and `sqlite-vec` (a vendored single C file, no Rust
  dep tree).
- The committed vector **sidecars are unchanged** — both the code and findings sidecars
  already live outside Lance.

## Capabilities

### Modified Capabilities

- `storage-backend-abstraction`: the single backend is now SQLite, not Lance.
- `lance-search`: text/embedding storage + BM25 ranking move to SQLite FTS5 + brute-force
  vectors (capability renamed to `code-search` is a follow-up, out of scope here).
- `index-store-db`: the snapshot's datasets are SQLite tables, not Lance datasets; column
  layouts and intern/uniqueness rules carry over unchanged.
- `findings-store`: the derived findings store is a SQLite database, not a Lance dataset;
  the committed `<id>.json` records and the findings sidecar are unchanged.

## Impact

- **Removed deps:** the 62-crate lance/df/arrow/parquet/sqlparser/object_store subtree.
  **Added:** `rusqlite` (bundled SQLite, ships FTS5 + trigram) and `sqlite-vec` (one vendored
  C file, no Rust dep tree) — a tiny net addition against a 62-crate removal.
- **Correctness gain, not just footprint:** the vector arm moves from Lance's *approximate*
  `IVF_PQ` to `sqlite-vec`'s *exact* brute-force — more correct nearest-neighbour results,
  well within the latency budget at kenn scale (brute-force is O(N), so ANN would only win at
  much larger corpora — deferred).
- **Expected build/binary win:** clean release build from ~14.5 min toward a few minutes
  (the 7-min LTO tail collapses — no LTO graph behind `rusqlite`'s bundled C); binary
  materially below 64 MB. **Validated by re-measuring after the port.**
- **Surface preserved:** `api::Reader`, `open_reader`, `open_writer`, `DbReader`,
  `DbWriter`, `WriteBatch`, and all row/result types are unchanged.
- **Migration:** none in-place. Snapshots are disposable build artifacts; the `backend`
  marker flips to `"sqlite"` and `schema_version` bumps, so old Lance snapshots are rejected
  with the standard "reindex required" message and the next `kenn index` writes SQLite. The
  committed sidecars and source corpus are untouched.
- **Out of scope:** changing search *semantics* (`mcp-symbol-search` and `search_findings`
  behaviour is preserved), the embedding model, the committed `<id>.json` finding records,
  or the collector store. The findings store's *storage engine* does change (it must);
  its externally-visible contracts do not.
