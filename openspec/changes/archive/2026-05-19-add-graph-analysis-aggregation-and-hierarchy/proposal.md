## Why

A prototype `kenn-analyze` crate already produces useful structural reports — god nodes, communities, per-community test ratios — but it recomputes the entire aggregated graph (symbol → enclosing aggregate, per-kind edge weights) on every invocation. Re-running clustering with different parameters pays for that pass each time, and external callers (MCP, future tooling) have no first-class artifact to query. The current flat Louvain output also conflates structure that a reader's mental model naturally separates: top-level subsystems (packages, crates) versus the inner sub-clusters within each one. Folding the projection into ingest and adding a hierarchical, anchored clustering view turns the aggregated graph into a snapshot artifact and gives readers a TOC-shaped report they can drill into.

## What Changes

- **New snapshot artifact:** the indexer's `end_run` computes the aggregated graph (rolled-up nodes + weighted undirected edges per kept kind) and persists it as two new redb tables, `aggregate_nodes` and `aggregate_edges`, alongside the existing tables.
- **Reader trait gains bulk-scan methods** for the aggregated graph (`scan_aggregate_nodes`, `scan_aggregate_edges`). Pre-existing per-symbol `scan_symbols` / `scan_edges` (added by the prototype) stay; the aggregated path becomes the fast lane for analysis.
- **`kenn-analyze` becomes a reader-only consumer** of those tables. The aggregation logic moves into `kenn-indexer`. Existing fallback: when a snapshot pre-dates the aggregate tables, `kenn-analyze` SHALL recompute on the fly (current behavior) and warn once.
- **Anchored hierarchical Louvain** replaces single-level clustering as the primary view. L0 = the symbol's package id (or first path segment of its def file when `pkg = 0`). L1+ = Louvain run independently within each anchor, recursing into communities until a size or modularity-gain threshold halts.
- **Flat Louvain** still runs over the whole aggregated graph and is rendered alongside the anchored hierarchy as a cross-check. Communities that span multiple anchors are flagged.
- **REPORT.md layout updates** to render: summary, three god-node sections (live/test/system — unchanged), per-anchor hierarchy with three buckets (live / mixed / test infra) per anchor, and the flat cross-check.
- **CLI surface** keeps `kenn analyze --top-n N`; gains `--max-depth N` (cap hierarchy depth, default 4) and `--min-cluster N` (min size to recurse, default 20).
- **BREAKING for `kenn-analyze`'s public Rust API**: `top_by_weighted_degree` and the `Projection` shape stay; `cluster::louvain` returns `Hierarchy` (new type) instead of `Vec<Vec<ShortId>>`. The flat partition is exposed as `Hierarchy::flat()`.
- Snapshot rollback to a pre-aggregate version: `kenn analyze` reports "not analyzed for this snapshot — recomputing" rather than crashing.

## Capabilities

### New Capabilities
- `graph-analysis`: aggregated-graph projection (roll-up rules, per-kind edge weights, kept/skipped edge kinds), anchored hierarchical clustering, flat clustering as cross-check, REPORT.md rendering, CLI surface, fallback behavior on pre-aggregate snapshots.

### Modified Capabilities
- `index-store-db`: adds `aggregate_nodes` and `aggregate_edges` redb tables to the schema and bumps the schema version; defines key/value layout and the end_run write order (after symbol/edge writes, before publish).
- `storage-backend-abstraction`: extends the `Reader` trait with `scan_aggregate_nodes` and `scan_aggregate_edges` methods; specifies behavior when the tables are missing (return empty, do not error).
- `scip-indexer`: adds an end-of-run aggregation step that reads the already-persisted symbol and edge tables to produce the aggregated-graph tables. The per-document SCIP and JSONL transforms are unchanged; the new step runs once per `kenn index` inside `end_run`, before snapshot publication.

## Impact

- **Code:** new module(s) in `kenn-indexer` for the projection pass; new tables and codecs in `kenn-store/src/backends/db_default`; trait additions in `kenn-store/src/api/reader.rs`; `kenn-analyze` loses its on-the-fly projection (kept behind a fallback path), gains `cluster::hierarchical` and a small anchored-Louvain implementation.
- **Schema version:** bump. Snapshots indexed under the previous version remain readable; `kenn analyze` falls back to the prototype's recompute path on those.
- **Performance budget:** the new end_run pass is O(N + E) over symbols + kept-kind edges. Must add < 10% to `kenn index` wall-time on a typical workspace; instrumented behind `KENN_BENCH`.
- **CLI / report:** `REPORT.md` shape changes (anchored hierarchy + flat cross-check). Existing consumers (humans) absorb the change; no programmatic dependency known.
- **Dependencies:** none added. Hierarchical clustering reuses the existing hand-rolled flat Louvain by running it on induced subgraphs at each level (see design.md for the rationale and the coarsening-based alternative considered).
- **MCP:** unchanged in this proposal. Tools that query the aggregated graph (`community_of`, `community_path`, `aggregate_neighbors`, `query_graph`) are deferred to a follow-up.
- **Out of scope (tracked as follow-ups):** MCP tools above; live/test split inside each community's member listing; discovered top-level as opt-in alternative to anchored; populating `pkg` on the Rust SCIP path so the file-path anchor fallback is no longer needed.
