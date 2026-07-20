## ADDED Requirements

### Requirement: Analysis tables in the snapshot schema

The snapshot schema SHALL include four new tables for persisted analysis. Each SHALL be written transactionally as part of the index run and SHALL be readable via the `Reader` trait alongside the existing `scan_aggregate_*` methods.

The tables (and corresponding `*Row` types in `kenn_store::api::types`):

1. **`analysis_god_nodes`** — top-N nodes by weighted degree.
   - Columns: `filter: GodNodeFilter`, `rank: u32`, `short_id: u32`, `weighted_degree: u64`, `name: String`, `kind: String`, `anchor_id: u32`, `anchor_name: String`.
   - `filter` is one of `live`, `test`, `external`; rows are sorted by `(filter, rank)`.
2. **`analysis_flat_communities`** — one row per flat-Louvain community.
   - Columns: `community_id: u32`, `size: u32`, `total_weight: u64`, `cross_anchor: bool`, `primary_anchor_id: u32`, `primary_anchor_name: String`.
   - `community_id` is dense (0..N) and deterministic for a given snapshot.
3. **`analysis_anchored_hierarchy`** — one row per node in the anchored hierarchical-Louvain tree (depth 0 = anchor, depth k > 0 = sub-community at recursion depth k).
   - Columns: `community_id: u32`, `parent_id: Option<u32>`, `depth: u32`, `anchor_id: u32`, `size: u32`, `test_ratio: f32`, `test_infra: bool`.
   - `parent_id` is `None` for depth-0 (anchor-root) rows.
4. **`analysis_node_membership`** — per-aggregate-node lookup.
   - Columns: `short_id: u32`, `flat_community_id: u32`, `anchored_leaf_community_id: u32`.
   - One row per aggregate node; rows sorted by `short_id`.

Row deserialization SHALL be derived via the existing `db_default` / `db_surreal` feature gates (matching the existing `AggregateNodeRow` pattern).

#### Scenario: Tables present after a successful index run

- **WHEN** `kenn index` completes successfully with `[index] persist_analysis = true`
- **THEN** `Reader::scan_analysis_god_nodes(filter)`, `scan_analysis_flat_communities()`, `scan_analysis_anchored_hierarchy()`, and `scan_analysis_node_membership()` MUST return non-empty results for any workspace with at least one anchored node

#### Scenario: Tables absent on legacy snapshots

- **WHEN** a snapshot was written by a pre-this-change `kenn index`
- **THEN** the analysis tables MUST behave as empty (return `Ok(vec![])` from each `scan_analysis_*` call, NOT panic or error)

### Requirement: Atomic analysis-write step

The analysis tables SHALL be written via a single `Writer::write_analysis(&AnalysisResult)` method (or equivalent batched API) that:

- Runs inside the index run's transaction; analysis rows commit atomically with the rest of the snapshot.
- Replaces any prior analysis rows for the snapshot in full (no partial-update semantics).
- Returns an error if the input `AnalysisResult` is internally inconsistent (a `node_membership` row references a `community_id` that doesn't exist in `flat_communities` or `anchored_hierarchy`).

#### Scenario: Re-index replaces prior analysis rows

- **WHEN** `kenn index --force` runs against a workspace whose snapshot already contains analysis tables
- **THEN** the new analysis rows MUST fully replace the prior rows
- **AND** no rows from the prior analysis MUST remain
