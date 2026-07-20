## Why

`get_workspace_overview` computes its counts at **runtime** —
`count_table("symbols")` / `count_table("files")` run `SELECT count(*)`
on every call — and offers no breakdown: `languages` is a bare list with no
sense of how much of each language the workspace holds, no split between
first-party and dependency code, and no signal of graph structure. The data
to answer "how many symbols/files/defs per language (and how much is
internal vs test vs external), how many packages per manager, and what shape
is the dependency graph" is all produced **at build** — during `finalize` and
the clustering pass. It should be **aggregated once at build** and stored, so
reads are a cheap lookup of precomputed stats, never live aggregation.

## What Changes

- **Add a `stats` table to `code.db`** — **narrow (long-format)**,
  `(scope, key, subset, metric, value)`, NOT a wide one-column-per-metric
  table. One row per `(scope, key, subset, metric)`, so new metrics,
  dimensions, or subsets need no schema change:
  - `scope='language', key='rust', subset='internal', metric='symbols', value=3100`
  - `scope='language', key='rust', subset='external', metric='symbols', value=800`
  - `scope='manager',  key='cargo', subset='external', metric='packages', value=37`
  - `scope='graph',    key='',      subset='all', metric='communities', value=42`
- **Split every entity count into three subsets** — `internal` (first-party,
  `external=0 AND test=0`), `test`, `external` (deps/stdlib) — plus an `all`
  union, so external/test code never silently dominates a language's number.
- **Populate entity counts at `finalize()`** via `GROUP BY (bucket, subset)`
  over the populated graph tables (symbols, files, defs per language;
  packages per manager) — same place `finalize` already derives `name_words`
  and the knowledge rows.
- **Collect graph-structure counters during clustering, not raw edges.** Raw
  `edges` are dropped (an edge spans two nodes / isn't symbol-sourced — a
  weak, unactionable number). Instead `kenn-analyze` records counts that fall
  out of building the clusters — communities, cross-anchor communities,
  god-nodes, anchored-hierarchy depth, node/anchor counts — under
  `scope='graph'`, written alongside the `analysis_*` tables.
- **One `write_stats(&[StatRow])` writer surface** both producers use;
  `INSERT OR REPLACE` by the row key.
- **Bump the store schema version** so pre-table snapshots reindex (the
  existing schema-mismatch → recovery path).
- **All aggregation at indexing; the read does none.** `finalize` stores the
  `global` grand totals too, so the overview reads precomputed rows only —
  no `SUM`, `count(*)`, `GROUP BY`, or `count_table` on the read path.
  `count_table` survives only on the empty-snapshot gate and cold-start
  reindex check ("are there *any* symbols?"), not the overview path.
- **Expose via the reader** (`stats()`, pooled) and **surface in
  `get_workspace_overview`**: per-language subset counts, per-manager package
  counts, and a graph-structure summary.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `index-store-db`: adds the `stats` table to the `code.db` schema and a
  `write_stats` writer op; bumps the schema version.
- `graph-analysis`: the clustering pass records graph-structure counters into
  `stats` as a byproduct of building the communities/hierarchy.
- `mcp-server`: `get_workspace_overview` reports per-language (subset),
  per-manager package, and graph-structure counts from the precomputed
  `stats` table.

## Impact

- **Code:** `crates/kenn-store/src/db/sqlite/schema.rs` (DDL + `names`
  registry + drift test), `…/writer/finalize.rs` + a `write_stats` writer
  method (`handle.rs`/`writer/*`), `…/reader/*` (a pooled `stats()` method +
  `StatRow`), the `STORE_SCHEMA_VERSION` constant,
  `crates/kenn-analyze/src/lib.rs` (counters in `build_analysis_hook`),
  `crates/kenn-mcp/src/tools/query.rs` + `types.rs` (`WorkspaceInfo`).
- **Behavior:** `get_workspace_overview` gains per-language subset counts,
  per-manager package counts, and a graph summary; existing scalar fields keep
  their meaning. Old snapshots reindex once on the schema bump. Wire shape of
  the overview's `languages` changes (updates the `mcp-offload` overview
  regression test + the `agent-guide` reference).
- **Dependency:** independent; composes with the pooled reader
  (`mcp-offload-blocking-storage`) but does not require it.
