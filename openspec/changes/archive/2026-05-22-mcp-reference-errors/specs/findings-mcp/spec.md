## MODIFIED Requirements

### Requirement: the MCP server exposes finding writes with provenance

The server SHALL expose `store_finding`, accepting `text`, `parent_ids`, and `tags`, and returning the new finding's id together with any semantically near existing findings. It SHALL expose `merge_findings`, which synthesizes a new finding from given finding ids and records those ids as parents.

Both SHALL validate their id inputs before writing. A `fnd_…` id that names no existing finding SHALL fail the call with `INVALID_INPUT`, and the error SHALL list **every** unresolved id, not only the first, so the caller corrects them in one round-trip. `merge_findings` inputs are findings, so every input id is checked. `store_finding`'s `parent_ids` mix finding ids and code-graph node ids; only the `fnd_…` ones are checked — a code-node reference is best-effort provenance whose later resolvability is reported by finding staleness, not enforced at write time.

#### Scenario: store_finding returns id and near-duplicates

- **WHEN** `store_finding` is called and a semantically similar finding already exists
- **THEN** the response contains the new finding's id
- **AND** the response lists the similar prior finding

#### Scenario: merge_findings records its inputs as parents

- **WHEN** `merge_findings` is called with two finding ids
- **THEN** a new finding is created whose `parent_ids` include both inputs

#### Scenario: unknown finding inputs are rejected, all at once

- **WHEN** `store_finding` or `merge_findings` is called with two `fnd_…` ids that name no existing finding
- **THEN** the response is an `INVALID_INPUT` error
- **AND** the error message names both unresolved ids

### Requirement: the MCP server exposes derivation-DAG traversal

The server SHALL expose `find_predecessors` and `find_successors`, walking the `parent_ids` edges of the unified ID space so a caller can trace a finding back to the code or earlier findings it was derived from.

The start id SHALL be validated: a `fnd_…` id that names no existing finding SHALL fail the call with `INVALID_INPUT` rather than return an empty walk. A code-node start id is accepted without a code-graph lookup — a code node has no predecessors, and `find_successors` from a refactored-away node must still reach the findings that cite it.

#### Scenario: provenance is walkable to source

- **GIVEN** a finding derived from another finding that cites a code-graph node
- **WHEN** `find_predecessors` is walked transitively from the finding
- **THEN** the walk reaches the originating code-graph node

#### Scenario: an unknown finding start id is rejected

- **WHEN** `find_predecessors` or `find_successors` is called with a `fnd_…` id that names no existing finding
- **THEN** the response is an `INVALID_INPUT` error
