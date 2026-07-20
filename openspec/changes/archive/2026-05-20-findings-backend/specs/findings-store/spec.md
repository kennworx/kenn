## ADDED Requirements

### Requirement: A finding is a durable, provenance-bearing record

The store SHALL persist a finding as a record carrying a unique `id`, free-form `text`, an `embedding`, a list of free-form `tags`, a list of `parent_ids`, and a `created_at` timestamp. `parent_ids` SHALL be drawn from a single ID space shared with code-graph nodes, so a parent MAY be a code-graph node or another finding.

The store SHALL NOT impose a fixed `kind` enumeration on findings; classification is carried by `tags`.

#### Scenario: a finding round-trips with its provenance

- **WHEN** a finding is stored with text, tags, and `parent_ids` referencing a code-graph node and an earlier finding
- **THEN** retrieving it by `id` returns the same text, tags, and `parent_ids`

### Requirement: store_finding persists a finding and returns its id

`store_finding` SHALL accept `text`, `parent_ids`, and `tags`, persist the finding, and return its `id`.

Surfacing semantically near-duplicate findings is **deferred**: it depends on the embedding producer that `lance-search-backend` left as a follow-up and that does not yet exist. Until that producer lands, `store_finding` returns only the new `id`; near-duplicate detection is a follow-up alongside vector search.

#### Scenario: a finding is persisted and its id returned

- **WHEN** `store_finding` is called with text, tags, and `parent_ids`
- **THEN** it returns the new finding's `id`
- **AND** the finding is retrievable by that `id`

### Requirement: findings are searchable by lexical query

`search_findings` SHALL return findings ranked by BM25 over `text`, served from the same engine as code search. Results SHALL be deterministic for a fixed query and corpus.

Vector / semantic ranking over `embedding` is **deferred**: it depends on an embedding producer that `lance-search-backend` left as a follow-up and that does not yet exist. The `embedding` column is reserved on the finding record and left null; `search_findings` is lexical-only until the producer lands, at which point this requirement becomes hybrid lexical + vector.

#### Scenario: a finding is retrieved by a lexical query

- **WHEN** `search_findings` is called with a query sharing terms with a stored finding's `text`
- **THEN** that finding appears in the ranked results

### Requirement: the derivation DAG is traversable

The store SHALL expose `find_predecessors` and `find_successors` over a finding, walking `parent_ids` edges. Because a finding may only reference earlier-created findings, the derivation graph SHALL be acyclic.

#### Scenario: provenance traces to source evidence

- **GIVEN** a finding whose parents include another finding that in turn references a code-graph node
- **WHEN** `find_predecessors` is walked transitively
- **THEN** the walk reaches the originating code-graph node
- **AND** the walk terminates (no cycle)

### Requirement: findings are append-only; corrections supersede and deletions tombstone

The store SHALL NOT modify a finding in place. A correction SHALL be a new finding carrying the prior finding in `parent_ids` and a `supersedes` tag; retrieval SHALL prefer the latest finding in a supersede chain. A deletion SHALL be a tombstone finding referencing the target; retrieval SHALL exclude tombstoned findings from normal results.

Because every finding file is write-once and uniquely named, a `git merge` of two branches that each added findings SHALL union them with no conflict.

#### Scenario: a correction supersedes without mutating

- **WHEN** a finding is corrected by storing a new finding that supersedes it
- **THEN** the original finding is still retrievable by `id`
- **AND** a default `search_findings` returns the superseding finding, not the original

#### Scenario: two branches add findings and merge cleanly

- **GIVEN** branch A and branch B each stored new findings
- **WHEN** branch B is merged into branch A with a plain `git merge`
- **THEN** the merge completes with no conflict
- **AND** the merged store contains the findings from both branches

### Requirement: staleness is computed at read time

The store SHALL NOT persist a staleness flag on a finding. At query time, the store SHALL check whether a finding's code-graph `parent_ids` still resolve in the current branch's code graph; if any do not, the result SHALL be flagged stale. A stale finding SHALL still be returned, marked, not omitted.

#### Scenario: a finding over removed code is flagged, not deleted

- **GIVEN** a finding whose evidence is a code-graph node
- **WHEN** the code graph is rebuilt on a branch where that node no longer exists, and the finding is queried
- **THEN** the finding is returned with a stale flag

#### Scenario: the same finding is live on a branch where the code remains

- **WHEN** the same finding is queried on a branch where its evidence node still exists
- **THEN** the finding is returned without a stale flag

### Requirement: findings reach the committed store via an explicit flush

`store_finding` SHALL write to a pending area. Findings SHALL enter the committed store on an explicit flush. The default flush policy SHALL commit every pending finding the caller has not explicitly dropped.

#### Scenario: pending findings are committed on flush

- **WHEN** several findings are stored and then a flush is invoked without dropping any
- **THEN** all of them appear in the committed store
- **AND** they are present after a fresh open of the store

#### Scenario: a dropped finding is not committed

- **WHEN** a pending finding is explicitly dropped before flush
- **THEN** it does not appear in the committed store
