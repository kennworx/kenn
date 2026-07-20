## Context

`get_workspace_overview` (`crates/kenn-mcp/src/tools/query.rs`) calls
`count_table("symbols")`, `count_table("files")`, and `distinct_languages()`
on every invocation; `count_table` is a live `SELECT count(*)`. There is no
per-language breakdown. The writer's `finalize()`
(`crates/kenn-store/src/db/sqlite/writer/finalize.rs`) already derives data
from the populated graph tables (the `knowledge` rows, the `name_words`
index), so it is the natural home for a one-time stats aggregation.

## Goals / Non-Goals

**Goals:**
- Per-language counts (symbols, files, defs, edges) and per-manager package
  counts are computed once at build and stored in `code.db`.
- The overview reads precomputed stats instead of aggregating live.
- Adding a new metric or dimension later needs no schema migration.

**Non-Goals:**
- Removing `count_table` (the empty-snapshot gate and whole-table totals keep
  using it).
- Incremental stats maintenance — stats are rebuilt with the snapshot, like
  every other derived table.

## Decisions

### D1: Narrow (long-format) `stats` table, with a subset dimension

```sql
CREATE TABLE stats (
  scope  TEXT NOT NULL,   -- 'language' | 'manager' | 'global'
  key    TEXT NOT NULL,   -- bucket: 'rust', 'cargo', … ('' for global)
  subset TEXT NOT NULL,   -- 'internal' | 'test' | 'external' | 'graph'
  metric TEXT NOT NULL,   -- 'symbols' | 'files' | 'defs' | 'packages' | 'communities' | …
  value  INTEGER NOT NULL,
  PRIMARY KEY (scope, key, subset, metric)
);
```

One row per `(scope, key, subset, metric)`. A new metric, dimension, or
subset is just new rows — no `ALTER TABLE`, no wide column sprawl. (`subset`,
not `set` — `SET` is a SQL keyword.)

**The `subset` dimension** is the lens on a count:
- `internal` — first-party code (`external=0 AND test=0`),
- `test` — test code (`external=0 AND test=1`),
- `external` — dependencies / stdlib / vendored (`external=1`),
- `graph` — a clustering/structure counter (see D-graph), not a code subset.

For entity metrics the first three are a disjoint partition (precedence
`external` → `test` → `internal`). There is **no** `all` subset — it carried
no information beyond the partition; a consumer that wants a grand total adds
the three values in code (a handful of integer adds when shaping the
response, not a DB aggregation). `subset='graph'` only ever pairs with the
graph metrics, so the column is unambiguous per metric.

*Alternative considered:* a wide `language_stats(language, symbols_internal,
symbols_test, …)` typed table. Rejected — every new metric or subset is a
schema change and new columns; packages (no language) don't fit a
row-per-language. The long form makes "split into 3 sets" free.

### D2: Entity counts at `finalize()` — `GROUP BY (bucket, subset)`

Insert rows from straight aggregations over the graph tables, on the writer's
`graph` connection inside `finalize`. One row per `(bucket, subset)` — no
`all` row:

- `language` × `symbols`:
  `SELECT language, CASE WHEN external=1 THEN 'external' WHEN test=1 THEN 'test' ELSE 'internal' END AS subset, count(*) FROM symbols GROUP BY language, subset`.
- `language` × `files`: same `CASE` over `files` (it carries `external`/`test`).
- `language` × `defs`: `defs d JOIN symbols s ON s.id=d.sym_id`, the `CASE`
  over the **symbol's** flags, grouped by `s.language`.
- `manager` × `packages`: `packages` carries `external` (no `test`), so the
  subsets are `internal` (workspace package) and `external` (dependency);
  grouped by `manager`.

Raw `edges` are not a per-language metric — an edge spans two nodes that may
be two languages and isn't always symbol-sourced (`Contains` is module→file),
so a per-language edge count is neither meaningful nor reconcilable with a
whole-table total. Graph structure is captured by D-graph instead.

### D-graph: graph-structure counters from clustering, `subset='graph'`

The **clustering pass** (`kenn-analyze`) contributes graph-shape counters —
real structural evidence (modularity, hubs, nesting), already computed when
the analysis records are built. They are written with `subset='graph'`, under
the scope they naturally attribute to. Most are **per-language** (the nodes
they count carry a language); two are inherently whole-graph:

Per language (`scope='language', subset='graph'`), each attributed to the
**plurality language of its member nodes** (an anchor/community spans
languages and has none itself; anchor ids are a different id space from node
ids, so member-language attribution is the correct route):
- `metric='nodes'` — aggregate nodes of that language;
- `metric='god_nodes'` — high-centrality hubs of that language (god-node
  `short_id` is a node id, so attributed directly);
- `metric='anchors'` — anchors whose plurality member language is that language;
- `metric='communities'` — flat communities whose plurality member language is
  that language (via the per-node `membership` records).

Whole-graph (`scope='global', key='', subset='graph'`):
- `metric='hierarchy_depth'` — `max(depth)` of the anchored hierarchy;
- `metric='cross_anchor_communities'` — communities spanning >1 anchor (by
  definition cross-language, so not attributable to one).

In `build_analysis_hook`, right after `to_records`, the counters are tallied
from the same `AnalysisResult` / `AnalysisRecords` the pass already produces
(cf. the existing `AnalysisCounts`) — no extra graph traversal — and written
via `write_stats`. The analysis pass is **optional**; when skipped the
`subset='graph'` rows are simply absent (overview shows no graph block) while
the `finalize` entity counts are always present.

### D-write: one generic `write_stats` writer surface

Both producers — `finalize` (entity counts) and the analysis hook (graph
counters) — write through a single `DbWriter::write_stats(&[StatRow])` that
`INSERT OR REPLACE`s rows by `(scope, key, subset, metric)`. They emit
disjoint rows, so they compose without coordination and the call is idempotent
on re-run.

### D3: No grand-total rows; totals summed in code, never on the DB

There is no `all`/`global` *entity* total row — it duplicated the partition.
A consumer that wants a workspace total adds the `internal`/`test`/`external`
values it already fetched (a few integer adds in code). The `global` scope is
used **only** for the two whole-graph `subset='graph'` counters. This keeps
*database* aggregation entirely at build time: the read path never runs
`count(*)`, `SUM`, or `GROUP BY` — it fetches the small `stats` set once and
reshapes it.

### D4: One pooled `stats()` reader method; overview reshapes the rows

The reader gains `stats() -> Vec<StatRow{scope,key,subset,metric,value}>`,
dispatched through the connection pool like every other read.
`get_workspace_overview` makes **one** `stats()` call and reshapes the rows:
`language`-scope rows → per-language blocks (subset-keyed counts + per-language
graph counters), `manager`-scope rows → per-manager package counts,
`global`/`graph` rows → the graph summary. The scalar `symbol_count` /
`file_count` are the in-code sum of the language `symbols`/`files` subset
rows. No DB aggregation on the read path.

`count_table` is no longer on the overview path. It survives only where it
answers a different question — "does this snapshot have *any* symbols?" — on
the empty-snapshot gate in `with_db` and the cold-start reindex decision in
`orchestrate.rs`; both keep their single-value `count(*)` presence check.

### D5: Schema-version bump

Adding the table changes the `code.db` schema, so the store schema version is
bumped. Snapshots built before this change fail the version check and route
through the existing schema-mismatch → reindex recovery path
(`index-store-db`: "Schema version bump"). No data migration is written;
reindex regenerates the snapshot including `stats`.

## Risks / Trade-offs

- [EAV-style table is less self-describing than typed columns] → bounded:
  `scope`/`metric` are a small closed vocabulary, documented in the `names`
  registry; the overview is the only structured consumer.
- [Edge attribution by source language is a choice] → documented; a
  `metric='edges_in'` could be added later with no migration if needed.
- [Schema bump forces a one-time reindex] → expected and already the standard
  mechanism for any schema change.

## Open Questions

- Should `defs`/`edges` per language ship in v1 or just `symbols`/`files` +
  `packages`? Leaning all-in (the narrow table makes it free), matching the
  "collect all" intent — revisit if finalize cost is measurable.
