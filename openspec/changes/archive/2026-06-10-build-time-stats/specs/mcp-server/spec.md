## ADDED Requirements

### Requirement: get_workspace_overview reports per-language stats

`get_workspace_overview` SHALL report, all read from the precomputed `stats`
table:
- **per-language** counts (scope `language`): for each language, its
  `symbols`, `files`, and `defs` counts split by `subset`
  (`internal`/`test`/`external`), plus its per-language graph counters
  (`subset='graph'`: `nodes`, `god_nodes`, `communities`, `anchors`) when the
  analysis pass ran;
- **per-manager** package counts (scope `manager`, subsets `internal`/`external`);
- a **whole-graph summary** (`scope='global', subset='graph'`:
  `hierarchy_depth`, `cross_anchor_communities`) when present.

These counts come from the build-time `stats` table, not from a live
aggregation on the read path.

The existing scalar fields (`snapshot_id`, `indexed_at`, `file_count`,
`symbol_count`, `packages`, `config_hint`) remain present with their current
meaning; the `languages` field SHALL carry the per-language stat blocks rather
than a bare list of language names. The scalar `symbol_count` / `file_count`
SHALL be the in-code sum of the `symbols` / `files` subset rows (a handful of
integer adds). The overview SHALL perform no **database** aggregation on the
read path — no `SUM`, `count(*)`, `GROUP BY`, or `count_table` query — it only
reshapes the rows `stats()` returns.

#### Scenario: Overview carries per-language breakdown

- **GIVEN** a Ready server over a snapshot indexed in more than one language
- **WHEN** the agent calls `get_workspace_overview`
- **THEN** the response lists each language with its `symbols`/`files`/`defs`
  counts (split by subset) from the `stats` table
- **AND** package counts are reported per manager
- **AND** when the analysis pass ran, a graph-structure summary is included

#### Scenario: Counts are precomputed, not aggregated on read

- **WHEN** `get_workspace_overview` builds its payload
- **THEN** every count comes from a row already in the `stats` table (the
  scalar totals being an in-code sum of the fetched subset rows)
- **AND** the overview runs no `SUM`, `count(*)`, `GROUP BY`, or `count_table`
  query on the read path
