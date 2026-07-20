## Context

`kenn-store` persists each index run as a snapshot under `runs/<ts>/lance/` (twelve+ graph
datasets + a `knowledge` search dataset), published by atomically retargeting the `live`
pointer. Reads go through `api::Reader` (impl'd by `DbReader`); the MCP server and CLI never
name the engine. Graph traversal does **not** run in Lance — `memory_graph.rs` bulk-scans
the edge/def/file tables once at open and builds an in-memory CSR projection that serves all
traversal. Lance's load-bearing jobs are therefore: (1) bulk row storage + scan-at-open,
(2) FTS/n-gram identifier + prose search, (3) a vector column (today searched as f32; the
IVF index is dormant). Vectors are canonically stored in a committed int8 sidecar outside
Lance and materialized into the search dataset's f32 column.

A spike (`tmp/sqlite-spike/REPORT.md`) confirmed SQLite covers all three and quantified the
prize (see `proposal.md`). This design ports the backend; it changes no `api` signature.

## Goals / Non-Goals

**Goals:**

- Replace the Lance engine with SQLite behind the unchanged `api::Reader` / factory surface.
- Preserve search *behaviour*: identifier ranking (exact-match boost + length tiebreak),
  blended name+doc scoring, and vector results equivalent to today's f32 search.
- Drop the lance/datafusion/arrow dependency subtree; cut build time + binary size.

**Non-Goals:**

- Changing MCP search semantics or tool contracts (`mcp-symbol-search` is preserved).
- Changing the committed vector sidecar format, the embedding model, or `incremental-embedding`.
- An ANN index. Brute-force is sufficient at single-repo scale (spike: 19 ms @1M int8);
  `sqlite-vec`/HNSW is a future option only if a corpus demands it.
- In-place data migration. Snapshots are disposable; reindex produces SQLite.
- Renaming the `lance-search` capability to `code-search` (cosmetic follow-up).

## Decisions

### D1 — Three independently-published SQLite files; atomic publish unchanged

Each snapshot writes **three** SQLite databases mirroring today's independently-swapped Lance
dirs: `graph.db` (the code graph), `knowledge.db` (the code search store), and `findings.db`
(the derived findings store). They are **not** merged into one file because `jobs.rs`
republishes them independently — the background embed job rebuilds and `publish_swap`s
*only* the knowledge store once vectors are filled, and findings stage/publish on their own
cadence. A single monolithic `index.db` would force rewriting the whole graph to swap in
filled vectors. Publish keeps today's two-level mechanism: a run-level `live` symlink points
at the whole snapshot `runs/<ts>/`, and an individual store is republished by
`publish_swap` — an **atomic `rename`** of a freshly-built file over the live one
(`rename(target→swapout); rename(build→target)`), exactly as `jobs.rs` does today for the
Lance `knowledge/` dir. So the embed job replaces `knowledge.db` in place inside the live run
without touching `graph.db`. Because `publish_swap` renames a fresh file over the old one and
**never writes a file a reader has open** (POSIX keeps the old inode alive for open fds — a
property `jobs.rs` already relies on), **WAL is unnecessary**; readers open
`mode=ro`/`immutable=1`. The `meta.json` marker is kept; `backend` flips from `"default"`
(Lance) to `"sqlite"`, `schema_version` bumps, and `check_backend_marker` +
`check_schema_version` reject old Lance snapshots with the standard "reindex required" error.

### D2 — Graph tables are SQLite tables; the in-memory CSR projection is unchanged

`symbols`, `defs`, `files`, `edges`, `packages`, `aggregate_*`, `analysis_*` become SQLite
tables with the same columns and the same intern/uniqueness *policy* (enforced by the writer,
as today — Lance enforced none either). The open-time projection in `memory_graph.rs` swaps
its Lance scan for a `SELECT * ` per table; the CSR build and all traversal logic are
untouched. Point fetches (`fetch_symbol_*`, `fetch_file_*`) become indexed primary-key
lookups (`CREATE INDEX` on the short-id / `(language, pub_id)` keys).

### D3 — Identifier search: FTS5(trigram) + replicated ranking

A `name_fts` FTS5 virtual table with the `trigram` tokenizer reproduces the n-gram
substring retrieval. To match kenn's ranking (the spike's 74.7% raw overlap was a ranking
difference, not a retrieval gap), the query layer applies kenn's policy on the FTS5
candidates: exact-name match boosted (the current 10×), then `(score DESC, len(name) ASC,
id ASC)`. `search_symbols_by_name` / `find_symbol_tiered` keep their signatures and tiering.

### D4 — Prose / doc search: FTS5(porter/unicode61), incl. file docs

A `doc_fts` FTS5 table with a stemming tokenizer provides BM25 over doc text, preserving the
`doc_text` BM25 arm of blended search. Both symbol-doc rows and `file_docs`
(path-identified, `embed_key = filedoc:<lang>:<path>`) rows are indexed, so
`search_blended_hits`' file-level doc hits are preserved. `search_symbols_blended` /
`search_blended_hits` fuse the FTS5 name + doc + vector arms with the existing fusion policy.

### D5 — Vectors: `sqlite-vec` `vec0`, exact brute-force (replaces Lance's approximate ANN)

Vector search uses **`sqlite-vec`** — a single-file C extension with no Rust dependency tree,
loaded into the SQLite we already bundle. Vectors live in a `vec0` virtual table; KNN is
`… WHERE embedding MATCH :q ORDER BY distance LIMIT k`, exact brute-force, SIMD-accelerated.

**The parity path stores `vec0 float[768]`** (the sidecar int8 dequantized to f32). Brute-force
over those f32 values is *exact w.r.t. the stored vectors* — the int8 storage quantization is
identical to today's (Lance reused the same dequantized sidecar values), so this is strictly
the exact NN over the same vectors Lance's index approximated. `sqlite-vec`'s `int8` and `bit`
`vec0` types are an **optional compact/fast path, not the exact path**: searching a re-quantized
representation trades recall for speed/space, so they stay behind a flag and are not claimed as
exact.

This is a **correctness upgrade, not parity**: kenn's *current* vector arm is Lance `IVF_PQ`
— an **approximate** ANN index (built whenever ≥256 vectors exist, which every real snapshot
has). Exact brute-force returns the true nearest neighbours, so its results will *deliberately
differ from* — and improve on — Lance's approximation. The spike showed exact brute-force is
fast enough at kenn scale (sub-2 ms @100k; 19 ms @1M scalar), so trading the ANN index for
exactness costs nothing here. No HNSW/IVF index is introduced; a pure-Rust ANN path
(`instant-distance`) stays named only as a deferred escape hatch for a hypothetical >1M-vector
corpus. A hand-rolled brute-force loop remains the zero-dependency fallback if we ever want to
drop even the C extension.

Registering `sqlite-vec` is one FFI call (`sqlite3_auto_extension(sqlite3_vec_init)`); it
needs a single narrowly-scoped `#[allow(unsafe_code, reason = "…")]` against the workspace's
`unsafe_code = "deny"`.

The **embedding lifecycle is preserved**, not collapsed into a finalize-time fill: `kenn
index` writes the search store with no vectors; reconciliation reuses committed sidecar
vectors by fingerprint at index time; and the background `embed_pending_into` / `reembed_into`
job embeds the remainder, appends a sidecar segment, and **republishes `knowledge.db`** (the
file-swap of D1). All three paths populate the `vec0` table instead of a Lance `embedding`
column.

### D5b — Findings store: a second SQLite store, same shape

The derived findings store moves to its own `findings.db`: rebuilt from the committed
`.kenn/findings/<id>.json` records (gitignored, as today), with FTS5 over finding text and a
`sqlite-vec` `vec0` table reconciled from the **separate** findings sidecar
(`.kenn/findings/vectors/`). `search_findings` keeps its hybrid lexical+vector behaviour and
signature. The committed `<id>.json` records, the findings sidecar format, and
`stage_findings_for_publish`'s independent publish are unchanged — only the derived store's
engine changes.

### D6 — Drop lance/df/arrow; add rusqlite (bundled)

Remove `lance*`, `datafusion*`, `arrow*`, `parquet`, `sqlparser`, `object_store` from
`kenn-store` (and any re-exporters). Add `rusqlite { features = ["bundled"] }` — ships
SQLite with FTS5 + the trigram tokenizer, no system dependency. A snapshot DB is written
once then read-only, so WAL is unnecessary for snapshots (single-writer build, then publish).

### D7 — `api::Reader` surface preserved; `SqliteReader` / `SqliteWriter` behind the factories

`open_reader` / `open_writer` return the same `DbReader` / `DbWriter` aliases, now backed by
SQLite. Sync `rusqlite` calls are wrapped at the impl boundary (the trait is async; the
trait doc already prescribes `spawn_blocking` for a sync engine). No caller in `kenn-mcp` /
`kenn-indexer` changes — enforced by the existing "compiles against the Reader trait only"
scenarios.

### D8 — Migration is a reindex

No data migration. `schema_version` bumps and the `backend` marker flips to `"sqlite"`, so
`check_backend_marker` / `check_schema_version` reject an old Lance snapshot with the standard
"reindex required" error; the next `kenn index` writes SQLite. Source corpus and the committed
sidecars are untouched, so the reindex is structural-only + vector reuse as today.

### D9 — Bulk ingest discipline

Ingest writes in one transaction with prepared statements (the indexer batches per language,
as today via `WriteBatch`). Throughput is validated against a full reindex of a real corpus
(target: not worse than ~2× the Lance ingest) before claiming done.

## Risks / Trade-offs

- **Ranking parity is per-arm, and means different things per arm.**
  - *Identifier / BM25 arm (D3):* the spike's 74.7% overlap must climb to near-parity after
    replicating the exact-match boost + length tiebreak. Gate: top-k overlap vs the Lance
    fixtures ≥ ~0.9; iterate the ranking SQL until met.
  - *Vector / blended arm (D5):* this is **not** an overlap gate. SQLite's `vec0` is exact;
    Lance's `IVF_PQ` is approximate, so divergence from the Lance fixtures is *expected and
    correct*. Validate instead that the exact results are sensible (known nearest-neighbour
    cases, monotonic distances) — penalising SQLite for differing from an approximation would
    be backwards.
- **Parity-baseline sequencing.** The parity test needs the Lance output to diff against, but
  the migration deletes Lance. Mitigation: **capture the Lance top-k outputs as committed test
  fixtures before removing the lance deps** (tasks order this explicitly). The BM25 arm is
  gated against those fixtures; the vector arm uses them only as a sanity reference.
- **Bulk-ingest throughput (D9).** Row-store inserts vs Lance columnar append. Mitigation:
  one-transaction + prepared statements + a throughput gate; SQLite commonly does ≥10⁶ rows/s.
- **Large-corpus vector latency (D5).** Brute-force is linear. Mitigation: f32 is fine to
  ~hundreds of k; int8 + SIMD/rayon extends it; `sqlite-vec`/HNSW is the escape hatch if a
  real corpus ever needs it (explicitly deferred).
- **Loss of DataFusion SQL / columnar scans.** kenn doesn't query in Lance (D2: traversal is
  in-memory), so this is not load-bearing; the only columnar consumer was the open scan,
  which a `SELECT` covers.

## Migration

None in-place (D8). Out-of-band: old `runs/*/lance/` snapshots become dead and may be GC'd;
the first post-swap `kenn index` writes `index.db`. `mcp` instances re-resolve `live` and
open the SQLite snapshot via the unchanged `open_reader`.

## Closeout note — the parity gate (tasks 4.4 / 5.3) was re-scoped

The "Parity-baseline sequencing" risk above became load-bearing in a way the plan didn't
anticipate, so the gate shipped differently than tasks 4.4 / 5.3 first described.

`fixtures/lance_baseline.json` (task 0.1) was captured from the **live Lance index of kenn's
own source**, and the backend port (commit `af7c7dd`) **refactored `kenn-store` itself**
between the freeze and the swap — `kenn-store::db::reader`, `db::graph::reader`, etc. all
moved under `db::sqlite::`. So the baseline's `kenn-store::*` `pub_id`s no longer exist in the
tree. Re-indexing with SQLite and diffing top-k against the fixture would report low overlap
from **corpus self-drift, not a ranking regression**, and Lance is deleted so no matching
baseline can be re-captured. An overlap-vs-Lance gate is therefore unrunnable.

Resolution (the goal — "search returns the right things in the right order" — was always the
point; Lance was only the incumbent proxy for it):

- **Code search (4.4):** `tests/search_ranking_parity.rs` asserts the ranking *policy* D3/D4
  promise on a fixed in-test corpus that never drifts — exact-match boosted to rank 0, trigram
  substring retrieval, identifier search is name-only, the doc arm surfaces doc-only matches
  below name matches, the ≥3-char trigram floor, and deterministic ordering. The vector arm is
  exact (D5), validated by NN sanity in `hybrid_search.rs`, not overlap.
- **Findings (5.3):** covered by the existing `tests/findings.rs`
  (`supersede_tombstone_and_staleness` exercises `search_findings` ranking behaviour); the
  baseline never contained findings data (its note says so).
- **`lance_baseline.json` is retained** as a captured historical reference, not a wired gate.

Observation worth recording: the `len(name) ASC` secondary sort key (D3) is largely **dormant**
under FTS5 `bm25`, which already length-normalizes — equal scores across different name lengths
(the only case the key discriminates) don't arise unless an upstream component equalizes them,
and the lone exact-match boost applies to a single symbol. The key remains as the documented
deterministic fallback (the `(score, len, id)` total order), but it is no longer a primary
ranking lever the way it was on Lance's non-normalized n-gram scores.

## Validation results — the prize, measured (task 8.3)

Measured on this machine after the swap, against the `proposal.md` "before" figures:

| Metric | Before (Lance, proposal.md) | After (SQLite) | Note |
|---|---|---|---|
| lance/df/arrow/parquet/sqlparser/object_store crates | 62 | **0** | `cargo tree -p kenn-cli` — directly comparable |
| `kenn-cli` normal-dep crates (total) | — | 195 | +`rusqlite` (bundled) +`sqlite-vec`; the 62-crate subtree is gone |
| Release binary (`kenn`) | 64 MB | **11 MB** | release profile, same binary — directly comparable (~83% smaller) |
| `kenn-cli` clean rebuild + fat-LTO link | ~7 min critical-path tail (of a 14m27s cold build) | **1m09s**, 1.8 GB peak RSS | **not** directly comparable: `cargo clean -p kenn-cli` + sccache warm, so it re-runs the kenn-cli compile + the fat-LTO link rather than a cold full build. The link that existed "almost entirely to optimize the lance/df graph" is now ~1 min. |

The binary-size and dep-subtree deltas are the clean, methodology-matched prizes. A true cold
full-workspace build time (comparable to 14m27s) was not re-measured — it would require purging
the shared sccache cache, which costs the user's other work for a number the binary/dep deltas
already evidence.

Not measured — **task 8.2 (bulk-ingest throughput, gate "≤ ~2× Lance")**: the Lance reference
is deleted, so the *relative* gate is unrunnable, the same way the parity baseline became
unrunnable. A standalone SQLite ingest-throughput figure can still be captured if a regression
is ever suspected, but there is no Lance number left to divide by. Left unchecked rather than
silently passed.

**Real-corpus end-to-end smoke (post-closeout).** Beyond the synthetic-corpus integration
tests, a full live pass was run on this workspace: `kenn index` produced a `backend: "sqlite"`
snapshot (`graph.db` 4.9 MB + `knowledge.db` 26 MB, no `lance/` dir; 265 docs / 4986 symbols /
5135 defs / 21089 edges), `kenn status` reported `success`, and `kenn mcp` opened it read-only
and served `find_symbol "CodeGraphNodeResolver"` → exact hit at
`crates/kenn-store/src/db/findings/graph_resolver.rs#15`. So indexing a real repository and
serving identifier search over MCP work on the SQLite engine, and the index reflects current
(post-rename) source.
