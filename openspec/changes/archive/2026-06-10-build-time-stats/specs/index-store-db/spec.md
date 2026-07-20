## ADDED Requirements

### Requirement: stats table holds build-time aggregate counts

The `code.db` schema SHALL include a narrow (long-format) `stats` table that
holds counts aggregated **at build**, one row per `(scope, key, subset, metric)`:

```sql
CREATE TABLE stats (
  scope  TEXT NOT NULL,   -- 'language' | 'manager' | 'global'
  key    TEXT NOT NULL,   -- bucket within the scope ('' for global)
  subset TEXT NOT NULL,   -- 'internal' | 'test' | 'external' | 'graph'
  metric TEXT NOT NULL,   -- 'symbols' | 'files' | 'defs' | 'packages' | 'communities' | …
  value  INTEGER NOT NULL,
  PRIMARY KEY (scope, key, subset, metric)
);
```

The table SHALL be long-format (NOT one column per metric) so a new metric,
dimension, or subset adds rows, not columns. For entity metrics the `subset`
column SHALL be the disjoint partition `internal` (`external=0 AND test=0`),
`test` (`external=0 AND test=1`), `external` (`external=1`). There SHALL be no
`all`/union subset row — a grand total is the sum of the partition, computed by
the consumer, never stored. `subset='graph'` is reserved for clustering
counters (see the graph-analysis capability) and pairs only with graph metrics.

The writer SHALL expose a `write_stats` operation that inserts/replaces rows by
`(scope, key, subset, metric)`, used by both build-time producers (the finalize
entity counts and the analysis graph counters).

During `finalize`, the writer SHALL populate the entity counts by aggregating
the graph tables (ALL database aggregation at build, none on the read path):
- `scope='language'` × {`symbols`, `files`, `defs`} per `language`, one row per
  subset (defs joined to their symbol; the subset `CASE` over the symbol's
  flags);
- `scope='manager'` × `packages` per `manager`, subsets `internal`/`external`
  (packages carry no `test`).

The `scope='global'` bucket SHALL hold only the whole-graph `subset='graph'`
counters; there are no global entity-total rows.

Adding this table is a schema change and SHALL bump the store schema version
(see "Schema version bump"), so snapshots built before it reindex via the
existing schema-mismatch recovery path.

#### Scenario: Per-language subset counts populated at finalize

- **GIVEN** a snapshot built with first-party, test, and external symbols in a
  language
- **WHEN** `finalize` completes
- **THEN** `stats` has `(scope='language', key=<lang>, subset='internal'|'test'|'external', metric='symbols')`
  rows with the respective counts
- **AND** no `subset='all'` row exists

#### Scenario: Per-manager package counts populated

- **GIVEN** a snapshot whose packages span more than one manager
- **WHEN** `finalize` completes
- **THEN** `stats` contains `(scope='manager', key=<manager>, subset='internal'|'external', metric='packages')`
  rows per manager

#### Scenario: stats is narrow, not wide

- **WHEN** the `stats` table is created
- **THEN** it has the fixed columns `(scope, key, subset, metric, value)` and
  adding a new metric or subset introduces new rows, not a new column
