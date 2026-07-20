## ADDED Requirements

### Requirement: An unresolved entity reference is an error, not an empty result

A tool that takes an entity reference — a symbol `pub_id`, a file, or a finding id — SHALL return an `INVALID_INPUT` JSON-RPC error when that reference does not resolve in the live snapshot, rather than a success payload with an empty result set. An empty `items` array is reserved for a reference that *resolves* but genuinely has no matches (for example, a real symbol with no callers).

Tools that return an explicit `{found: false}` payload — `get_symbol`, `get_source`, `get_finding` — satisfy this requirement as-is: `{found: false}` is unambiguous. Search tools — `search_symbols`, `find_symbol`, `semantic_search`, `search_findings` — are exempt: an empty result is the correct answer to a query that matched nothing.

`find_at_location` SHALL address its file by `file_path`, a workspace-relative or absolute path; a path absent from the snapshot SHALL be an `INVALID_INPUT` error. No numeric file id SHALL appear on the tool surface — a per-run `short_id` carries no snapshot-stable meaning and would be a silent staleness hazard.

#### Scenario: navigating from a non-existent symbol id

- **WHEN** the agent calls `list_callers` with an `id` that resolves to no symbol
- **THEN** the response is an `INVALID_INPUT` error naming the id
- **AND** it is not an empty success payload

#### Scenario: a resolved symbol with no matches returns empty

- **WHEN** the agent calls `list_callers` for a symbol that exists but nothing calls
- **THEN** the response is a success payload with an empty `items` array

#### Scenario: find_at_location on an unindexed file

- **WHEN** the agent calls `find_at_location` with a `file_path` not present in the snapshot
- **THEN** the response is an `INVALID_INPUT` error
