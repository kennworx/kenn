## MODIFIED Requirements

### Requirement: code graph persisted as Lance datasets

The code-graph store — `symbols`, `defs`, the per-kind edge data, the `aggregate_*` and `analysis_*` data — SHALL be persisted as Lance datasets. The backend SHALL NOT use redb or any storage engine other than Lance. The code graph SHALL remain a throwaway, gitignored, per-branch artifact, rebuilt by `kenn index`; only its storage engine changes. Its location SHALL be the configured derived-store root (`Layout::derived_root`), which defaults to `.kenn/local/` and MAY be relocated — including to a global folder shared across branches.

Search-lookup columns SHALL carry Lance scalar BTREE indexes — among them the symbol-name column, `pub_id`, `short_id`, and `path` — serving equality and range lookups and batched `take()` hydration. Low-cardinality filter columns (`kind`, `language`, `external`, `test`, edge kind) SHALL carry Lance BITMAP indexes. Edge data SHALL NOT carry a scalar index — graph traversal reads it by bulk scan, not per-vertex query.

A dataset below a small-corpus row-count threshold MAY skip its scalar indexes: a full scan of so few rows already falls within the query-planning floor, so the index would not earn its build cost. The threshold is an implementation detail; on any non-trivial workspace every indexed dataset carries its indexes.

#### Scenario: the code graph is a Lance store

- **WHEN** `kenn index` completes a run
- **THEN** the code-graph store on disk consists of Lance datasets
- **AND** no `.redb` file is produced
- **AND** `redb` does not appear in the `kenn-store` dependency tree

#### Scenario: code graph honors the configured derived root

- **WHEN** `[layout] derived_root` is set away from the default
- **AND** `kenn index` completes a run
- **THEN** the code-graph Lance datasets are written under that derived root
- **AND** nothing of the code graph is written under `committed_root`
