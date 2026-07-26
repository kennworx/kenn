## MODIFIED Requirements

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
  `hierarchy_depth`, `cross_anchor_communities`, `domains`) when present.

The whole-graph summary's `cross_anchor_communities` is the RAW clustering
counter — every community spanning more than one anchor, before any selection —
and SHALL NOT be presented as the workspace's domain count. `domains` is the
EARNED count: the communities that clear the axis floors, which is what the atlas
renders and what a domains query returns. Both SHALL be reported, each named for
what it is, so neither can be mistaken for the other.

NEITHER counter bounds the other, and a consumer SHALL NOT assume an ordering
between them. They range over different candidate sets: a repo one package
dominates reports `cross_anchor_communities = 0` — nothing spans two anchors —
while `domains` stays non-zero, because the axis deliberately keeps within-anchor
clusters for a monolithic library.

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

#### Scenario: The raw counter cannot be read as the domain count

- **GIVEN** a snapshot whose partition yields more cross-anchor communities than
  clear the domain floors
- **WHEN** the agent calls `get_workspace_overview`
- **THEN** both the raw counter and the earned `domains` count are reported
- **AND** they are distinguishable by name, not by the reader's inference
