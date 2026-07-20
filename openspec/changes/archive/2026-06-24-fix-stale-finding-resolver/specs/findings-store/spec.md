## MODIFIED Requirements

### Requirement: staleness is computed at read time

The store SHALL NOT persist a staleness flag on a finding. At query time, the store SHALL check whether a finding's code-graph `parent_ids` still resolve in the current branch's code graph; if any do not, the result SHALL be flagged stale. A stale finding SHALL still be returned, marked, not omitted. The resolution SHALL key on the **canonical code-node id** — the `pub_id` as returned by `find_symbol` and stored in a finding's `parent_ids` (e.g. `rs:foo::bar`, `cs:Ns.Type`), which already carries the language short-code. The resolver SHALL NOT re-prefix it with the `language` column (`rust`/`csharp`/…); doing so doubles the id (`rust:rs:foo`) so it never matches and every code-cited finding falsely folds to stale.

#### Scenario: a finding over removed code is flagged, not deleted

- **GIVEN** a finding whose evidence is a code-graph node
- **WHEN** the code graph is rebuilt on a branch where that node no longer exists, and the finding is queried
- **THEN** the finding is returned with a stale flag

#### Scenario: the same finding is live on a branch where the code remains

- **WHEN** the same finding is queried on a branch where its evidence node still exists
- **THEN** the finding is returned without a stale flag

#### Scenario: a finding citing a present symbol by its canonical id is live

- **GIVEN** a finding whose `parent_id` is a symbol's canonical id (e.g. `cs:Ns.Type`) that exists in the current code graph
- **WHEN** the finding is queried
- **THEN** it is returned without a stale flag, because the resolver keys on the same canonical `pub_id`, not a language-doubled form
