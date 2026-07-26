# index-store-db

## Purpose

This spec defines the SurrealDB-backed snapshot schema that the indexer
consumer writes and the MCP server reads. It pins the table layouts,
column semantics, and uniqueness rules that connect the JSONL wire
frames to durable on-disk records — packages interned by
`(name, version)`, declaration sites split into a `defs` table, and
symbols disambiguated by `(pub_id, pkg)` rather than `pub_id` alone.
## Requirements
### Requirement: packages table interned by (name, version)

The schema SHALL define a `packages` SQLite table:

```
packages
  short_id   u32     PRIMARY
  name       string
  version    string
  manager    string
  external   bool    default false
```

The engine enforces no key uniqueness. `(name, version)` is the consumer's intern key, enforced as ingest policy by the consumer's intern logic — packages with the same `name` and `version` from different producer runs (or different on-wire `PackageFrame`s within one run) collapse to one row at intern time, before insert. `short_id` carries a SQLite index.

`version` and `manager` are non-nullable; an absent value is the empty string. Different `version`s of the same `name` produce distinct rows (`Newtonsoft.Json` v12 and v13 are separate packages).

#### Scenario: Two PackageFrames with identical (name, version) collapse

- **WHEN** the wire stream emits two `PackageFrame`s, both with `name: "Web"` and `version: "1.0"`
- **THEN** the `packages` table MUST contain exactly one row matching `name = 'Web' AND version = '1.0'`
- **AND** any `SymbolFrame` referencing either wire id MUST resolve to that single row

### Requirement: defs table for declaration sites

The schema SHALL define a `defs` SQLite table:

```
defs
  sym_id      u32                 // FK → symbols.short_id
  file_id     u32                 // FK → files.short_id
  start_line  u32
  start_col   u32
  end_line    u32
  end_col     u32
```

The `defs` table carries no per-symbol index: the reader bulk-scans it once at open into an in-memory map keyed by `sym_id` and serves declaration-site lookups from RAM, never a per-symbol query (design D3/D6).

One row per declaration site. The common case (non-partial symbol) produces one `defs` row per `symbols` row. Partial classes/methods produce N `defs` rows per `symbols` row, all sharing `sym_id`.

Lines and columns SHALL be stored as separate primitive columns so that callers projecting only line data (the `path#start-end` rendering case) can omit column projection.

#### Scenario: Non-partial symbol has exactly one defs row

- **WHEN** the consumer ingests a single `SymbolFrame` for a non-partial symbol
- **THEN** the `symbols` table MUST contain one row for that symbol
- **AND** the `defs` table MUST contain exactly one row with `sym_id` matching that symbol's `short_id`

#### Scenario: Partial class produces one symbols row and N defs rows

- **WHEN** the consumer ingests three `SymbolFrame`s with `partial: true`, distinct wire ids, and matching `(key, pkg)`
- **THEN** the `symbols` table MUST contain exactly one row for the symbol
- **AND** the `defs` table MUST contain three rows, all sharing `sym_id`, with `file_id`/`start_line`/`end_line` reflecting the three declaration sites

#### Scenario: Project line-only without column data

- **WHEN** an MCP query renders a symbol location as `path#start_line-end_line`
- **THEN** the query MUST be expressible as a projection of `file_id, start_line, end_line` from `defs` filtered by `sym_id`
- **AND** the column data MUST NOT be fetched

### Requirement: symbols table layout

The `symbols` table SHALL include:

```
symbols
  short_id          u32     PRIMARY
  pub_id            string  BTREE index (non-unique)
  language          string
  pkg               u32     default 0
  kind              string
  name              string
  name_lower        string  BTREE index — case-folded `name`
  enclosing_symbol  u32     default 0
  partial           bool    default false
  nargs             u32     default 0
  targs             u32     default 0
  external          bool    default false
  test              bool    default false
```

Declaration locations live in the `defs` table, and signature / doc
text in the `symbol_docs` table keyed by `symbol` (the symbol's
`short_id`) — neither is a column on `symbols`. `name_lower` is the
case-folded `name`, carrying the BTREE index that backs case-insensitive
symbol search; `enclosing_symbol` is the direct-parent `short_id`, `0`
for a top-level symbol.

The `pub_id` column has a non-unique SQLite **BTREE** index.
Different rows MAY share `pub_id` when they belong to different packages
(e.g., two versions of the same library that both declare
`Newtonsoft.Json.JsonConvert`). Uniqueness is `(pub_id, pkg)`,
enforced as ingest policy by the consumer's intern logic.

`pkg` is a non-nullable `u32` with `0` as the "no package" sentinel
value (matching `REF_NONE`). Consumers MAY filter `pkg != 0` when only
package-scoped symbols are of interest.

`external` is a denormalized boolean derived from
`packages[pkg].external` at insert time. Symbols with `pkg = 0` SHALL
have `external = false`.

#### Scenario: Two rows can share pub_id when packages differ

- **WHEN** the consumer ingests two `SymbolFrame`s with the same `key`
  but `pkg` resolving to two different `packages` rows
- **THEN** the `symbols` table MUST contain two rows
- **AND** both rows MUST have the same `pub_id` value
- **AND** the rows MUST differ in `pkg`

#### Scenario: external is derived from package

- **WHEN** the consumer inserts a symbol whose resolved package has
  `external: true`
- **THEN** the new `symbols` row's `external` column MUST be `true`

#### Scenario: pkg = 0 means no package

- **WHEN** a `SymbolFrame` arrives with `pkg` omitted
- **THEN** the corresponding `symbols` row MUST have `pkg = 0`
- **AND** `external` MUST be `false`

### Requirement: aggregate_nodes table

The default-backend schema SHALL define an `aggregate_nodes` SQLite table with one row per aggregate node:

```
aggregate_nodes
  short_id     u32      // same id as the underlying anchor symbol
  kind         string   // class / struct / trait / interface / enum / module / namespace / package
  name         string   // display name
  language     string
  external     bool     // mirrors the anchor symbol's external
  test         bool     // mirrors the anchor symbol's test
  example      bool     // the node's primary def lies under an example/sample/demo/fixture path
  anchor_id    u32      // interned anchor id (package short_id, or interned path-prefix id)
  anchor_name  string   // human-readable anchor label
```

The aggregate node's `short_id` SHALL be the `short_id` of the anchor symbol the projection rolled it up to (the nearest enclosing class-like or module-like symbol). This means aggregate nodes are a subset of `symbols`, and the same id space identifies a symbol and its corresponding aggregate node when one exists.

`external`, `test`, and `example` are the node's provenance flags. `example` SHALL be evaluated once, during aggregation, from the node's primary definition file path — the same path the anchor resolution already reads — and SHALL NOT be re-derived by consumers. A consumer that needs to know whether a node is example code SHALL read this column.

#### Scenario: Aggregated class is queryable by short_id

- **WHEN** a class with `short_id = 42` is the aggregate target for some method's roll-up
- **THEN** the `aggregate_nodes` table MUST contain a row with `short_id = 42`
- **AND** that row's `kind` MUST be `class`

#### Scenario: Symbol not chosen as any aggregate is absent

- **WHEN** a method `short_id = 99` rolls up to its enclosing class `42`
- **THEN** the `aggregate_nodes` table MUST NOT contain a row with `short_id = 99`

#### Scenario: A node defined under an example path is flagged

- **WHEN** an aggregate node's primary definition file is `crates/<pkg>/examples/spike.rs`
- **THEN** its `aggregate_nodes` row MUST have `example = true`
- **AND** a node defined under `crates/<pkg>/src/` MUST have `example = false`

#### Scenario: A snapshot written before the column is rejected, not misread

- **WHEN** a snapshot persisted under an older store schema version is opened
- **THEN** the store MUST report a schema mismatch and require a reindex
- **AND** MUST NOT surface a partially-read `aggregate_nodes` row

### Requirement: aggregate_edges table

The default-backend schema SHALL define an `aggregate_edges` SQLite table with columns `node_min: u32`, `node_max: u32`, `kind: u32` (the `EdgeKind` discriminant), and `weight: u32`. Each row represents one undirected aggregated edge of a specific kind between two aggregate nodes, with `node_min <= node_max`. Multiple kinds between the same pair of aggregates SHALL be stored as separate rows.

The engine enforces no key uniqueness. The aggregation writer SHALL deduplicate undirected edges before insert: it SHALL canonicalize each edge's endpoints (`min` then `max`), merge the weights of edges sharing `(node_min, node_max, kind)`, and emit exactly one row per distinct `(node_min, node_max, kind)`. There MUST NOT be two rows for the same `(node_min, node_max, kind)`, and there MUST NOT be two rows differing only by endpoint direction.

#### Scenario: Symmetric edge writes deduplicate in the aggregation writer

- **WHEN** the aggregation pass produces a `calls` edge between aggregates 5 and 10 (regardless of which is source and which is target)
- **THEN** the `aggregate_edges` table MUST contain exactly one row with `node_min = 5`, `node_max = 10`, `kind = calls as u32`

#### Scenario: Multiple kinds between same pair produce separate rows

- **WHEN** aggregates 5 and 10 are connected by both `calls` (weight 3) and `type_use` (weight 2)
- **THEN** the table MUST contain two rows: `(node_min = 5, node_max = 10, kind = calls, weight = 3)` and `(node_min = 5, node_max = 10, kind = type_use, weight = 2)`

### Requirement: Aggregate tables written during end_run

The indexer SHALL populate `aggregate_nodes` and `aggregate_edges` inside `end_run`, after all per-unit ingest is complete (all symbols, edges, and defs already in their tables), and before the snapshot is published as live. A failure during aggregate writes SHALL fail `end_run` and prevent snapshot publication — partial aggregate tables MUST NOT be observable to readers.

#### Scenario: Successful end_run publishes a snapshot with non-empty aggregate tables

- **WHEN** `kenn index` completes successfully on a workspace with at least one class and one inter-class call
- **THEN** the published snapshot's `aggregate_nodes` table MUST be non-empty
- **AND** the `aggregate_edges` table MUST be non-empty

#### Scenario: Aggregate-write failure prevents snapshot publication

- **WHEN** aggregate writes fail mid-`end_run` (simulated I/O error)
- **THEN** the previous live snapshot MUST remain unchanged
- **AND** the new snapshot's building directory MUST NOT be flipped to live

### Requirement: Schema version bump

The code-graph store's move to the SQLite snapshot database is a snapshot-layout break: a pre-SQLite on-disk format is not readable by the SQLite-backed reader. The reader SHALL detect a snapshot predating the SQLite code-graph layout, treat it as outdated, and trigger a rebuild from source; it SHALL NOT attempt to decode the older store, and SHALL NOT silently misread it. The code graph is a throwaway artifact, so a rebuild loses no irreproducible data.

Detection SHALL be structural rather than a numeric `SCHEMA_VERSION` constant: a SQLite-layout snapshot has the code-graph SQLite database with a `symbols` table, which a pre-SQLite snapshot does not. A reader opening a snapshot whose code-graph `symbols` table is absent SHALL report the code-graph store is outdated.

#### Scenario: a pre-SQLite snapshot triggers a rebuild

- **WHEN** a kenn binary opens a snapshot persisted under a pre-SQLite code-graph layout
- **THEN** the reader reports the code-graph store is outdated
- **AND** the snapshot is rebuilt from source
- **AND** no rows are served decoded from the older store

### Requirement: Analysis tables in the snapshot schema

The snapshot schema SHALL include four SQLite tables for persisted analysis. Each SHALL be written as part of the index run and SHALL be readable via the `Reader` trait alongside the existing `scan_aggregate_*` methods.

The tables (and corresponding `*Row` types in `kenn_store::api::types`):

1. **`analysis_god_nodes`** — top-N nodes by weighted degree.
   - Columns: `filter: GodNodeFilter`, `rank: u32`, `short_id: u32`, `weighted_degree: u64`, `name: String`, `kind: String`, `anchor_id: u32`, `anchor_name: String`.
   - `filter` is one of `live`, `test`, `external`; rows are sorted by `(filter, rank)`.
2. **`analysis_flat_communities`** — one row per flat-Louvain community.
   - Columns: `community_id: u32`, `size: u32`, `total_weight: u64`, `cross_anchor: bool`, `primary_anchor_id: u32`, `primary_anchor_name: String`.
   - `community_id` is dense (0..N) and deterministic for a given snapshot.
3. **`analysis_anchored_hierarchy`** — one row per node in the anchored hierarchical-Louvain tree (depth 0 = anchor, depth k > 0 = sub-community at recursion depth k).
   - Columns: `community_id: u32`, `parent_id: u32`, `depth: u32`, `anchor_id: u32`, `anchor_name: String`, `size: u32`, `test_ratio: f32`, `test_infra: bool`.
   - `parent_id` is a non-nullable `u32`; `0` is the no-parent sentinel for depth-0 (anchor-root) rows, matching the `0` / `REF_NONE` convention for absent ids elsewhere in the schema. Callers treat `(parent_id == 0 && depth == 0)` as the anchor-root.
4. **`analysis_node_membership`** — per-aggregate-node lookup.
   - Columns: `short_id: u32`, `flat_community_id: u32`, `anchored_leaf_community_id: u32`.
   - One row per aggregate node; rows sorted by `short_id`.

The reader materializes each `*Row` type from the table's columns; there are no backend-feature-gated serde derives.

#### Scenario: Tables present after a successful index run

- **WHEN** `kenn index` completes successfully with `[index] persist_analysis = true`
- **THEN** `Reader::scan_analysis_god_nodes(filter)`, `scan_analysis_flat_communities()`, `scan_analysis_anchored_hierarchy()`, and `scan_analysis_node_membership()` MUST return non-empty results for any workspace with at least one anchored node

#### Scenario: Tables absent on legacy snapshots

- **WHEN** a snapshot was written by a pre-this-change `kenn index`
- **THEN** the analysis tables MUST behave as empty (return `Ok(vec![])` from each `scan_analysis_*` call, NOT panic or error)

### Requirement: Atomic analysis-write step

The analysis datasets SHALL be written via a single `Writer::write_analysis(&AnalysisResult)` method (or equivalent batched API) that:

- Writes all analysis rows into the run's `building/` snapshot directory. The rows become observable to readers only when the snapshot is published by the atomic directory swap; a run that fails before publish leaves no observable analysis rows and the prior live snapshot unchanged.
- Replaces any prior analysis rows in full — the snapshot is rebuilt wholesale, so there are no partial-update semantics.
- Returns an error if the input `AnalysisResult` is internally inconsistent (a `node_membership` row references a `community_id` that doesn't exist in `flat_communities` or `anchored_hierarchy`).

#### Scenario: Re-index replaces prior analysis rows

- **WHEN** `kenn index --force` runs against a workspace whose snapshot already contains analysis datasets
- **THEN** the new published snapshot's analysis rows MUST fully replace the prior rows
- **AND** no rows from the prior analysis MUST remain

### Requirement: code graph persisted as SQLite tables

The code-graph store SHALL be persisted as tables in the SQLite snapshot database — namely
`symbols`, `defs`, the per-kind edge data, and the `aggregate_*` and `analysis_*` data. The
backend SHALL NOT use Lance, DataFusion, Arrow, redb, or any storage engine other than SQLite. The
code graph SHALL remain a throwaway, gitignored, per-branch artifact, rebuilt by `kenn index`;
only its storage engine changes. Its location SHALL be the configured derived-store root
(`Layout::derived_root`), which defaults to `.kenn/local/` and MAY be relocated — including to
a global folder shared across branches.

Search-lookup columns SHALL carry SQLite indexes — among them the symbol-name column,
`pub_id`, the volatile `id`, and `path` — serving equality and range lookups and batched
hydration. Low-cardinality filter columns (`kind`, `language`, `external`, `test`, edge kind)
MAY carry indexes where they earn their cost. Edge data SHALL NOT carry a per-vertex index —
graph traversal reads it by bulk scan into the in-memory CSR projection, not by per-vertex
query.

A table below a small-corpus row-count threshold MAY skip its indexes: a full scan of so few
rows already falls within the query-planning floor, so the index would not earn its build
cost. The threshold is an implementation detail; on any non-trivial workspace every indexed
table carries its indexes.

#### Scenario: the code graph is a SQLite store

- **WHEN** `kenn index` completes a run
- **THEN** the code-graph store on disk is a SQLite database
- **AND** no Lance dataset directory is produced
- **AND** none of `lance`, `datafusion`, `arrow`, or `redb` appears in the `kenn-store`
  dependency tree

#### Scenario: code graph honors the configured derived root

- **WHEN** `[layout] derived_root` is set away from the default
- **AND** `kenn index` completes a run
- **THEN** the code-graph SQLite database is written under that derived root
- **AND** nothing of the code graph is written under `committed_root`

### Requirement: code graph table layouts and intern rules are preserved

The column layouts, intern keys, and uniqueness policies of the core tables SHALL be
preserved unchanged across the engine swap: `packages` interned by `(name, version)`; `defs`
as declaration sites; `symbols` disambiguated by `(pub_id, pkg)`; the uniform id / FK column
naming (`id`, `pub_id`, `<role>_id`); and the `aggregate_*` / `analysis_*` schemas. As under
Lance, the engine enforces no key uniqueness — `(name, version)` and `(pub_id, pkg)` remain
ingest-policy intern keys enforced by the consumer before insert.

#### Scenario: intern keys carry over unchanged

- **WHEN** the wire stream emits two `PackageFrame`s with identical `(name, version)`
- **THEN** the `packages` table contains exactly one matching row
- **AND** any `SymbolFrame` referencing either wire id resolves to that single row

#### Scenario: id and FK column names are unchanged

- **WHEN** the graph tables are written
- **THEN** every entity table's own key column is `id`, stable identities are `pub_id`, and
  numeric foreign keys end in `_id` (e.g. `sym_id`, `file_id`, `pkg_id`, `src_id`, `target_id`)

### Requirement: Uniform id and FK column naming across core datasets

Across the core entity and relation datasets, id columns SHALL follow one rule: a table's own numeric, run-volatile key SHALL be named `id`; a stable, API-visible string identity SHALL be named `pub_id`; and a numeric foreign key SHALL be named `<role>_id`. The concrete column names SHALL be:

- `symbols`: `id` (was `short_id`), `pub_id` (unchanged), `pkg_id` (was `pkg`), `enclosing_sym_id` (was `enclosing_symbol`).
- `symbol_docs`: `sym_id` (was `symbol`).
- `defs`: `sym_id`, `file_id` (unchanged).
- `edges`: `src_id` (was `source`), `target_id` (was `target`), `corr_canon_id` (was `corr_canonical`).
- `files`: `id` (was `short_id`), `path` (unchanged — see exception below).
- `packages`: `id` (was `short_id`), `name` (unchanged — see exception below).
- `aggregate_nodes`: `id` (was `short_id`); `aggregate_edges`: `min_id`, `max_id` (were `node_min`, `node_max`).

`src_id` / `target_id` SHALL remain generic (not `sym_id`) because edge endpoints are polymorphic — they reference a symbol, file, or package depending on edge kind. The derived-analysis datasets are out of scope and SHALL be left unchanged.

#### Scenario: Entity keys and FKs use the convention

- **WHEN** the graph datasets are written
- **THEN** every entity table's own key column is `id`
- **AND** every numeric foreign key column ends in `_id` and names the role it references (e.g. `sym_id`, `file_id`, `pkg_id`, `enclosing_sym_id`, `src_id`, `target_id`, `corr_canon_id`, `min_id`, `max_id`)

### Requirement: Domain-meaningful stable identities are exempt from `pub_id`

Where an entity's stable, API-visible identity is already a domain-meaningful string, that column SHALL keep its domain name rather than be renamed to `pub_id`: `files.path` and `packages.name`. The package `(name, version)` pair remains an internal interning key and `version` is not promoted to an identity column.

#### Scenario: Files and packages keep their domain identity names

- **WHEN** the `files` and `packages` datasets are written
- **THEN** the file's stable identity column is `path` and the package's is `name`
- **AND** neither is renamed to `pub_id`

### Requirement: file_docs dataset stores per-file comment text

The consumer SHALL maintain a sparse `file_docs` dataset with one row per file that has a surviving file-level doc: `{ file_id, doc: <Utf8> }`, where `file_id` references `files.id`. A file with no surviving doc SHALL have no `file_docs` row (sparse, mirroring `symbol_docs`). The dataset SHALL be distinct from both `files` (docs are not a dense column on the file row) and `symbol_docs` (a file is not a symbol), leaving room to index file source bodies under separate storage in a future change.

#### Scenario: A file with a useful header gets one file_docs row

- **WHEN** a `FileFrame.doc` array survives license filtering with non-empty text
- **THEN** exactly one `file_docs` row is written keyed by the file's `id` (column `file_id`)
- **AND** its `doc` is the surviving entries joined with a blank-line separator

#### Scenario: A file whose only comment is a license header gets no row

- **WHEN** every entry in `FileFrame.doc` matches the license-boilerplate heuristic
- **THEN** no `file_docs` row is written for that file

### Requirement: License-boilerplate filtering on file-doc ingest

When converting a `FileFrame.doc` array to a `file_docs` row, the consumer SHALL drop entries that match a conservative license/boilerplate heuristic and keep the rest. The heuristic SHALL match (case-insensitively) at least: `SPDX-License-Identifier`, `Copyright (c)` / `Copyright ©`, `All rights reserved`, `Licensed under`, `Permission is hereby granted`, and the canonical opening phrasing of the GPL, Apache-2.0, and MIT licenses. Filtering SHALL be per-entry — a matching entry is dropped without discarding sibling entries.

#### Scenario: License entry dropped, purpose entry kept

- **GIVEN** a `FileFrame.doc` of `["// Copyright (c) 2026 Acme. All rights reserved.", "// Handles order intake validation."]`
- **WHEN** the consumer builds the file_docs row
- **THEN** the stored `doc` is `"// Handles order intake validation."` only

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

