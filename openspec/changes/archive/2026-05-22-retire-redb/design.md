## Context

The `db_default` backend runs two embedded engines. redb holds the
**code graph** — `symbols`, `defs`, per-kind `edge_*`, `aggregate_*`,
`analysis_*` tables, reached through composite-key B-tree range scans
(`db/key.rs`). Lance holds the **search / knowledge store** — text,
BM25, n-gram, vectors.

`lance-search-backend` D1 kept redb deliberately — *"Lance is columnar
and scan-oriented — the wrong shape for traversal"* — and split the
stores by lifecycle: the code graph throwaway, the knowledge store
git-committed. Both premises have since changed:

- **The lifecycle split is gone.** The completed `incremental-embedding`
  change moved the committed artifact into the `.kenn/vectors/` binary
  sidecar and inverted `.gitignore`. `.kenn/.gitignore` now lists
  `local/` *and* `knowledge/` — the Lance store is derived and
  gitignored, rebuilt per worktree, exactly like the redb snapshots.
  Neither the code graph nor the knowledge store is committed; the only
  committed artifact across the two engines is the non-database
  `.kenn/vectors/` sidecar.
- **The performance premise is wrong.** A benchmark spike (redb vs.
  Lance, 80k symbols / 839k edges) showed Lance has a ~175 µs per-query
  planning floor — index-independent
  (ZoneMap was no faster than BTree) — *but* the Lance-native pattern,
  one bulk scan into an in-memory CSR adjacency traversed in RAM, runs
  the full sweep in ~4 ms, faster than redb's ~65 ms.

So redb is now a second engine — and a bespoke, non-interoperable format
(`.redb` opens in one Rust crate; `.lance` opens in the whole Arrow
ecosystem) — for a store that is already derived and throwaway. There is
nothing left for the dual-engine split to buy.

## Goals / Non-Goals

**Goals:**

- One storage engine — Lance — for all of `kenn-store`. redb deleted.
- One derived store: every database `kenn` writes is a gitignored Lance
  dataset, rebuilt per worktree, openable by any Arrow tool.
- No behavior or data-model regression; no seconds-scale cost anywhere.

**Non-Goals:**

- The `.kenn/vectors/` embedding sidecar — committed, owned by
  `incremental-embedding`, untouched.
- Committing any Lance data to git. retire-redb commits no new Lance
  data — the code-graph and knowledge stores stay derived. The findings
  store is a pre-existing committed Lance store; making it derived is
  the separate `committed-findings` change.
- Forcing the heterogeneous code-graph tables into one Arrow schema.
- The `committed-findings` change — a parallel, consistent move (it
  likewise relocates a Lance store under gitignored `.kenn/local/` and
  strips git-merge machinery), but its implementation is out of scope
  here. The legacy SurrealDB backend is also untouched.

## Decisions

### D1 — One engine, one derived store

redb is deleted: the `redb` and `bincode` dependencies, `db/key.rs`,
`db/schema.rs`, `db/codec.rs`, and the redb reader/writer paths. The code
graph becomes Lance datasets under `.kenn/local/`, alongside the
knowledge store.

A Lance *dataset* binds exactly one Arrow schema, so the code graph's
heterogeneous tables (`symbols`, `defs`, edges, `aggregate_*`,
`analysis_*`) and the search rows remain **several Lance datasets** — not
one sparse mega-schema. "One store" means one engine, one on-disk
format, one location, one build, one gitignore disposition — *not* one
schema. That is the usable, ecosystem-readable store; schema sanity is
kept.

*Alternative — one wide row-kind-discriminated schema:* rejected;
cramming symbol, edge, and hierarchy rows into one nullable-column schema
is needless complexity. *Alternative — keep redb (`lance-search` D1):*
rejected; its lifecycle and performance premises no longer hold.

### D2 — One snapshot, one atomic publish

The two derived datasets a `kenn index` run produces — the code graph
and the knowledge store — are built into a single `building/` directory
and published by **one atomic directory swap** (`building/` →
`snapshots/<ts>/`, then the `live` symlink flip). Today the redb snapshot
and the knowledge store publish independently; they unify into one
per-index-run snapshot. The findings store is workspace-durable — rebuilt
on its own lifecycle, independent of the index run — and is not part of
this snapshot.

A reader therefore only ever observes a fully-built snapshot — there is
no window in which one dataset is populated and another empty. This is
what lets the old cross-engine non-atomicity caveat be dropped outright
rather than restated.

### D3 — Access-pattern discipline (the load-bearing constraint)

Lance's ~175 µs query-planning floor is paid once per `scan()`. The
design is correct only if it never issues a per-item query in a loop.
Two patterns are mandatory:

- **Traversal / analysis** SHALL bulk-scan the relevant dataset once into
  an in-memory structure — a CSR adjacency for the edge graph — and
  traverse in RAM. Never one query per vertex.
- **Hydration** (ids → full records) SHALL collect the id set and issue
  one batched `take()`. Never one `take()` / query per id.

A per-item Lance query loop is the single seconds-scale trap
(175 µs × N → 15 s at N = 80k). This rule is normative; it belongs on the
review checklist, and the `d3_access_pattern` test — which times both
D3-governed reader surfaces (the CSR build at `open_reader` and
`list_outbound`'s traversal + batched hydration) over a representative
40k-symbol code graph and asserts they stay an order of magnitude under
the per-item-loop cost — is its automated regression guard.

*Alternative — per-item scalar-index queries:* rejected; the 15 s path.
*Alternative — a faster scalar index:* rejected; the spike shows the
floor is the query path, not the index.

### D4 — Redb-shaped call sites are rewritten, not ported

| call site (today, redb) | becomes (Lance) |
|---|---|
| `reader.rs` loops `read_symbol` per search hit | collect `short_id`s → one batched `take()` |
| `kenn-analyze` range-scans `edge_*` per vertex | bulk-scan edges once → in-memory CSR → traverse |
| `find_symbol_tiered` exact/prefix tiers → `SYMBOLS_BY_NAME` | BTREE scalar query / range query on the name column |
| `write_batch` point-reads redb `SYMBOL_DOCS` for `sig` | resolve `sig` from in-memory build state |

`list_inbound` / `list_outbound` / `find_at_location` / aggregate scans
are served from the in-memory graph the reader loads at open time.

### D5 — Writer-side dedup replaces redb key collisions

redb deduplicated by composite-key collision — a second write to the
same key overwrote. Lance is append-only and enforces no key uniqueness;
two appends are two rows. Dedup moves into the writer:

- `aggregate_edges` — the aggregation pass (which already builds the
  projection in memory at `end_run`) canonicalizes each undirected edge's
  endpoints (`min`, `max`), merges weights for a shared
  `(node_min, node_max, kind)`, and emits exactly one row per triple.
- `symbols` / `files` / `packages` — `short_id`-`PRIMARY` tables. Here
  redb's key overwrite was only a *backstop*: the ingester's
  stub-buffering consumer already emits exactly one record per
  `short_id`. That exactly-once invariant is now **unbackstopped** — a
  regression that double-emits a `short_id` would silently yield
  duplicate rows instead of being absorbed — so `write_batch` carries a
  `debug_assert` that no `short_id` repeats within or across a run's
  batches.
- Any other place redb relied on key-collision dedup is audited and made
  explicit in the writer.

### D6 — Scalar index choices

- **BTREE** on the search keys — the name column, `pub_id`, `short_id`,
  `path`. Equality + range (range covers prefix); ~175 µs/query, within
  an interactive search budget. A dataset below a small-corpus row count
  skips the index — a full scan of so few rows is already within the
  planning floor.
- **BITMAP** on low-cardinality filter columns — `kind`, `language`,
  `external`, `test`, edge kind.
- **No scalar index on edge data** — traversal bulk-scans it (D3); an
  index would only serve the prohibited per-item query.
- ZoneMap, BloomFilter, RTree are not used (spike R2-C: ZoneMap did not
  beat the planning floor; RTree is spatial-only).

### D7 — Retire the custom commit / merge machinery

The `lance-search` custom `CommitHandler` (ULID manifest paths), the
git-merge handler, fragment renumbering, and index-preservation-across-
merge exist solely to make Lance files survive a `git merge`. The
search / knowledge store is no longer git-committed, so that machinery
is dead code *for it* and removed here — the search store takes Lance's
default manifest path.

The machinery itself is not deleted. The findings store
(`db/findings/`) is still a committed Lance dataset and keeps using it —
`CommittedManifestHandler` and `reconcile_after_merge` stay live there.
Making the findings store derived is the separate `committed-findings`
change (a Non-Goal here); only when it lands does `commit_handler.rs`
become deletable.

This also reconciles the `lance-search` capability spec. `incremental-embedding`
made the Lance store derived/gitignored in code but shipped without a
`lance-search` delta, so that spec still describes a git-committed,
merge-clean store with a custom `CommitHandler`. The `lance-search` delta
in this change removes those now-false requirements — including the two
(`committed embeddings survive…`, `reconciliation on rebuild…`) that
`incremental-embedding` superseded with its sidecar — so the spec ends
consistent with shipped reality.

This is in scope because leaving committed-store machinery on an
uncommitted store is exactly the half-done state this change exists to
eliminate. If the change grows too large in review it may be split into a
follow-up, but it is specified here.

### D8 — Snapshot layout break detection

The redb→Lance move is a snapshot-layout break: the redb on-disk format
is not readable by the Lance reader. Rather than a numeric
`SCHEMA_VERSION` constant — the redb `db/schema.rs` that held it is
deleted — the break is detected structurally: a Lance-layout snapshot
has a `symbols/` Lance dataset, a redb-era one does not. Opening a
snapshot without it reports "code-graph store outdated — rebuilding" and
rebuilds from source, never misreads. The code graph is throwaway, so a
rebuild loses no irreproducible data.

### D9 — Async surfaces, and the ingest channel is removed

redb is synchronous: `kenn-store` wraps storage calls in
`spawn_blocking` / `run_blocking`, the writer is a sync object owned by
one OS thread (the `kenn-indexer` DB-writer thread), and the language
ingesters fan into it over a bounded `sync_channel`. Lance is
async-native, so with redb gone:

- The `Reader` implementations drop the `run_blocking` redb wrappers and
  call Lance directly; the writer surface (`write_batch`, finalize)
  becomes async.
- **The ingester → DB-writer channel is removed.** Each language
  ingester owns its own async writer and appends its batches directly to
  the building datasets.

Inspection of `ingest_channel.rs` confirmed the channel is a pure funnel
— interning is already partitioned per language ingester, and the
DB-writer thread carries no ordering or dedup logic. It existed only
because a single synchronous writer had to be owned by one thread. Three
checks clear the way to delete it:

- **Concurrent commits are safe.** D7 restores Lance's default
  `CommitHandler`, whose optimistic guard resolves concurrent commits —
  `Append` is conflict-free with `Append`, so a manifest-race loser
  rebases and retries. The writer wraps `commit` in a thin retry loop if
  Lance surfaces the conflict rather than auto-retrying.
- **Compaction is unaffected** — verified: `finalize` runs a *single*
  `compact_files` pass after all ingest (`store.rs::finalize` →
  `compact()`), not one per batch or per committer. However many
  fragments the run produced, compaction is one post-ingest pass.
- **Backpressure stays bounded** — memory in flight becomes
  `(language count) × (per-ingester batch size)`. Language count is
  small, so this is a fine bound; the single 50k-record channel was just
  one way to cap the same thing.

Constraint: there is **one writer per language ingester**, not one per
parser thread — a language whose parsing fans out internally (e.g. a
parallel-project parser) still accumulates into that one ingester's
batch. This keeps the concurrent-committer count equal to the language
count. Clean-stream detection (the channel's `Begin`/`End` matching)
moves to the ingester tasks' join results.

## Risks / Trade-offs

- **A future per-item query loop silently regresses to seconds** → D3 is
  normative; the review checklist names it; the `d3_access_pattern` test
  (a corpus sized so a per-item regression trips the bound by ~3.5×) is
  the automated guard.
- **In-memory CSR residency** — the reader holds the edge graph in heap
  rather than relying on redb's mmap paging → ~tens of MB even at 10M
  edges; trivial. Symbol records are not held resident — they are
  `take()`-hydrated on demand.
- **Cold-start bulk scan** — the reader pays one full edge scan at open →
  ~ms in the spike, linear; tens of ms on a large monorepo, negligible
  beside the indexing/embedding work already on that path.
- **CSR build needs grouped edges** → write the edge dataset
  source-sorted in the single rebuild pass, or counting-sort on load (one
  extra O(E) pass, sub-ms).
- **Snapshot format break** — old redb snapshots are unreadable →
  mitigated by the D8 version check: explicit rebuild, never a misread.
- **Scope creep from D7** → the commit-machinery removal is delineated as
  its own decision and task group, severable if review demands.
- **Concurrent-commit contention during ingest** → bounded: the
  committer count equals the (small) language count, `Append`-vs-`Append`
  retries are metadata-only, and compaction is a single post-ingest pass
  regardless (D9). If a future ingester design pushes the committer count
  high, revisit.

## Migration Plan

This change MUST be archived after `incremental-embedding`: its
`incremental-embedding` and `lance-search` spec deltas modify or remove
requirements that enter `openspec/specs/` only when `incremental-embedding`
archives.

1. Land the Lance code-graph datasets, the unified `building/` snapshot
   (D2), and the in-memory CSR reader behind the existing `db_default`
   feature — no parallel backend, no dual-write.
2. Rewrite the D4 call sites; move dedup to the writer (D5); delete
   `key.rs`, `schema.rs`, `codec.rs`, the redb paths, and the D7 commit /
   merge machinery; drop the `redb` and `bincode` dependencies.
3. Bump the layout version (D8).
4. Rollback: revert the change. The code graph is rebuildable from
   source, so rollback loses no data; a re-index restores a redb snapshot
   under the reverted code.
