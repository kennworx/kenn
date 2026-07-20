## MODIFIED Requirements

### Requirement: findings are searchable by hybrid lexical + vector query

`search_findings` SHALL return findings ranked by a combination of BM25 over `text` and vector similarity over `embedding`. Results SHALL be deterministic for a fixed query and corpus.

The search SHALL be served from a **persistent** index built outside the read path — created from the committed records when the store opens and maintained as findings are written — NOT a transient index built per call. The read path MUST NOT create a table or build an index. The lexical stage SHALL push its limit into the persistent index query so the candidate set is capped, and the result set SHALL be resolved to full records only for the top-`limit` hits (no full-corpus record load). Lifecycle (superseded / tombstoned) SHALL be filtered within the index query.

#### Scenario: a finding is retrieved by meaning

- **WHEN** `search_findings` is called with a query that paraphrases a stored finding without sharing exact terms
- **THEN** that finding appears in the ranked results

#### Scenario: no index is built on the read path

- **WHEN** `search_findings` is called
- **THEN** it queries the persistent findings index
- **AND** it does NOT create a table or build an index, and resolves only the top-`limit` records rather than loading every finding
