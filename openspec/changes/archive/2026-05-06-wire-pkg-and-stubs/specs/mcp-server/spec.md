## ADDED Requirements

### Requirement: get_symbol tolerates non-unique pub_id

`get_symbol(pub_id)` SHALL NOT assume `pub_id` is unique across the
`symbols` table. The DB schema permits multiple rows with the same
`pub_id` when they belong to different packages.

Until the in-flight MCP-server redesign replaces the surface, the
internal implementation SHALL return the first matching row (ordered
by `short_id ASC`) and MUST NOT panic, error, or otherwise fail when
multiple rows match. The returned `SymbolRef` envelope SHALL include
the resolving package's `name` and `version` so the agent can detect
the multi-match case and follow up.

The concrete tool-level behavior for the multi-match case (return all,
require `pkg` to disambiguate, prefer one over another) is owned by
the MCP-server redesign and is not pinned by this proposal. This
proposal commits only to (a) the data-model invariant that
`(pub_id, pkg)` uniquely identifies a symbol, and (b) that the
existing tool surface keeps working under the relaxed uniqueness.

#### Scenario: Multi-version package does not crash get_symbol

- **WHEN** the workspace transitively depends on two versions of the
  same package, each declaring a symbol with the same `pub_id`
- **AND** the agent calls `get_symbol(id)` with that `pub_id`
- **THEN** the call MUST succeed
- **AND** MUST return one of the matching rows
- **AND** the response envelope MUST identify the resolving package

### Requirement: Locations rendered as path#startLine-endLine

Locations returned by MCP tools SHALL be rendered in the form
`<file_path>#<start_line>-<end_line>` using line numbers from the
`defs` table. Column data is not included in the default rendering.

When an agent needs precise column ranges (e.g., for highlighting a
specific identifier within a line), tool implementations MAY include a
secondary structured field carrying the four-tuple
`(start_line, start_col, end_line, end_col)` from `defs`. This is an
optional extension; the default surface stays line-only.

For partial symbols, the response SHALL include all declaration sites
from `defs` (one rendered location per site).

#### Scenario: Default rendering uses line range only

- **WHEN** an MCP tool returns a symbol's location
- **THEN** the rendered form MUST match
  `<path>#<start_line>-<end_line>`
- **AND** column numbers MUST NOT appear in the rendered string

#### Scenario: Partial symbol returns multiple locations

- **WHEN** the agent calls `get_symbol(id)` for a symbol with
  `partial = true` and three `defs` rows
- **THEN** the response MUST include three rendered locations,
  one per declaration site
