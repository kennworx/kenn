## RENAMED Requirements

- FROM: `### Requirement: store_finding persists a finding and returns its id`
- TO: `### Requirement: store_finding persists a finding and reports near-duplicates`

- FROM: `### Requirement: findings are searchable by lexical query`
- TO: `### Requirement: findings are searchable by hybrid lexical + vector query`

## MODIFIED Requirements

### Requirement: store_finding persists a finding and reports near-duplicates

`store_finding` SHALL accept `text`, `parent_ids`, and `tags`, persist the finding, return its `id`, and additionally return any existing findings whose content is semantically similar above a threshold. The store SHALL NOT auto-merge or auto-discard on similarity — it SHALL return the matches and leave the decision to the caller.

#### Scenario: a similar prior finding is surfaced

- **GIVEN** a finding semantically close to one already stored
- **WHEN** `store_finding` is called
- **THEN** it returns the new finding's `id`
- **AND** it returns the similar prior finding among its results

### Requirement: findings are searchable by hybrid lexical + vector query

`search_findings` SHALL return findings ranked by a combination of BM25 over `text` and vector similarity over `embedding`, served from the same engine as code search. Results SHALL be deterministic for a fixed query and corpus.

#### Scenario: a finding is retrieved by meaning

- **WHEN** `search_findings` is called with a query that paraphrases a stored finding without sharing exact terms
- **THEN** that finding appears in the ranked results
</content>
