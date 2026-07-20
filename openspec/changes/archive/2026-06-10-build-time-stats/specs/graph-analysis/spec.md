## ADDED Requirements

### Requirement: clustering records graph-structure counters into stats

The analysis pass (`kenn-analyze`) SHALL record graph-structure counters into
the `stats` table with `subset='graph'` when it builds the community/centrality
clusters for a snapshot. The counters SHALL be derived from the same
`AnalysisResult` / `AnalysisRecords` the pass already produces (no extra graph
traversal), and written via the `write_stats` writer operation alongside the
`analysis_*` tables.

Counters that attribute to a language (their nodes carry one) SHALL be recorded
**per language** (`scope='language'`, `key=<language>`):
- `nodes` — aggregate nodes of that language;
- `god_nodes` — high-centrality hub nodes of that language;
- `anchors` — anchors of that language;
- `communities` — flat communities whose plurality member language is that
  language (an anchor/community spans languages and has none itself).

Counters that describe the whole partition SHALL be recorded once
(`scope='global'`, `key=''`):
- `hierarchy_depth` — maximum depth of the anchored hierarchy;
- `cross_anchor_communities` — communities spanning more than one anchor.

Raw per-language edge counts SHALL NOT be recorded — an edge spans two nodes
that may be two languages and is not always symbol-sourced, so the count is
neither meaningful per language nor reconcilable with a whole-table total.

The analysis pass is optional in the pipeline; when it does not run, the
`subset='graph'` rows are absent and consumers treat them as unavailable (the
entity counts from `finalize` are unaffected).

#### Scenario: Per-language graph counters written during analysis

- **GIVEN** indexing runs with the analysis (clustering) pass enabled
- **WHEN** the pass writes the `analysis_*` tables
- **THEN** `stats` contains `(scope='language', key=<lang>, subset='graph', metric='god_nodes'|'communities'|'nodes'|'anchors')`
  rows per language
- **AND** `(scope='global', key='', subset='graph', metric='hierarchy_depth')`
  and `cross_anchor_communities` rows
- **AND** no raw per-language `edges` rows are written

#### Scenario: Analysis skipped leaves entity counts intact

- **GIVEN** indexing runs without the analysis pass
- **WHEN** the snapshot is published
- **THEN** `stats` has the `finalize` entity-count rows (language/manager)
- **AND** has no `subset='graph'` rows
