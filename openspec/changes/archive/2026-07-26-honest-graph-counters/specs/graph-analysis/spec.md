## MODIFIED Requirements

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
- `cross_anchor_communities` — communities spanning more than one anchor. This is
  the RAW clustering diagnostic: every such community, before any selection. It
  SHALL keep this meaning. The earned domain count is a SEPARATE counter written
  by the aggregation stage (see "the aggregation stage records the earned domain
  count"), because the earned-span rule lives in `kenn-indexer` and this pass may
  not depend on it.

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
- **AND** `(scope='global', key='', subset='graph', metric='hierarchy_depth')` and
  `cross_anchor_communities` rows
- **AND** no raw per-language `edges` rows are written

#### Scenario: Analysis skipped leaves entity counts intact

- **GIVEN** indexing runs without the analysis pass
- **WHEN** the snapshot is published
- **THEN** `stats` has the `finalize` entity-count rows (language/manager)
- **AND** has no `subset='graph'` rows

## ADDED Requirements

### Requirement: the aggregation stage records the earned domain count

The aggregation stage (`kenn-indexer`) SHALL record the EARNED cross-package
domain count as a `(scope='global', key='', subset='graph', metric='domains')`
stat row: the communities that clear the domain axis's floors, where a package
joins a community's span only with enough members AND a first-party edge to
another qualifying package. This is the number the atlas renders and a domains
query returns.

It SHALL be computed by the SAME implementation of the earned-span rule that the
atlas producer and the domains query use, so a third surface cannot report a
different answer for one snapshot.

The aggregation stage SHALL compute it by reading the persisted community tables
(`analysis_flat_communities`, `analysis_node_membership`) back on its own writer
connection — never by recomputing clustering, and never by depending on
`kenn-analyze`, which the atlas capability already forbids. Writing it here
rather than in the analysis pass is what keeps that constraint intact while still
producing the row on every path.

The row SHALL NOT be conditional on the atlas bundle being built — a counter
present only on runs that rendered the atlas is a worse contract than the
inconsistency it replaces. It SHALL be written only when clustering produced
communities, so an absent row means the analysis pass did not run, which is
exactly when `cross_anchor_communities` is also absent.

`domains` and `cross_anchor_communities` are distinct questions over different
candidate sets, and NEITHER bounds the other. No ordering invariant between them
may be asserted: a multi-package repo typically reports far fewer earned than
raw, while a single-package repo reports `cross_anchor_communities = 0` with a
non-zero `domains`, because the axis deliberately keeps within-anchor clusters
for a repo that one package dominates.

#### Scenario: The earned count matches what the axis reports

- **GIVEN** a snapshot whose analysis pass ran
- **WHEN** the `domains` stat row is compared with what a domains query returns
  for that same snapshot, and with the count the atlas index header states
- **THEN** all three agree
- **AND** they agree because they share one implementation of the rule, not
  because separate copies happen to match

#### Scenario: The earned count is written without an atlas

- **GIVEN** indexing runs with clustering enabled but the atlas bundle disabled
- **WHEN** the snapshot is published
- **THEN** the `domains` stat row is still present

#### Scenario: A single-package repo inverts the two counters

- **GIVEN** a repo in which one package holds the majority of eligible nodes
- **WHEN** the counters are recorded
- **THEN** `cross_anchor_communities` MAY be `0` while `domains` is non-zero
- **AND** neither is treated as an error, because the domain axis keeps
  within-anchor clusters for such a repo
