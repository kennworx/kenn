## ADDED Requirements

### Requirement: def_range line basing is 1-based, column basing is 0-based

The `defs` table's `start_line` and `end_line` columns SHALL hold **1-based** line numbers (the editor convention: the first line of a file is line 1). The `start_col` and `end_col` columns SHALL hold **0-based** column numbers (the column of the first character on a line is column 0).

Ingest is the single boundary where this conversion happens. Producer wire formats — SCIP `Occurrence.range` and the dotnet `def_range` JSONL field — are 0-based on both axes (their native conventions). Each ingest transform SHALL add `+1` to `start_line` and `end_line` before pushing a `DefRecord`. Columns SHALL be stored as received.

Downstream consumers (MCP wire renderers, `find_at_location`, `get_source`) SHALL consume stored values directly with no further basing adjustment. Tool *inputs* that name a source line — today this is `find_at_location.line` — SHALL also be **1-based** so that values pasted from stack traces, compiler errors, editor "go to line", and prior MCP responses (`get_source.start_line`, wire `#<line>` format) round-trip without translation. The MCP tool description SHALL document this explicitly.

#### Scenario: A symbol declared on file line 16 stores start_line = 16

- **WHEN** a SCIP `Occurrence` for a definition reports `range = [15, 4, 15, 18]` (0-based)
- **THEN** the resulting `DefRecord` in the store MUST have `start_line = 16, start_col = 4, end_line = 16, end_col = 18` (lines `+1`, columns unchanged)
- **AND** `get_source` rendering that symbol MUST return the text of line 16 of the file

#### Scenario: dotnet JSONL frame with 0-based range stores 1-based lines

- **WHEN** a C# `symbol` frame arrives with `def_range = [9, 13, 9, 16]` (0-based, per `dotnet-stream-indexer`)
- **THEN** the resulting `DefRecord` MUST have `start_line = 10, start_col = 13, end_line = 10, end_col = 16`

#### Scenario: find_at_location accepts a 1-based line

- **WHEN** a function's declaration occupies file line 1868 and the agent calls `find_at_location(file_path, line=1868)`
- **THEN** the response MUST include that function as the smallest enclosing symbol
- **AND** a call with `line=1867` (the blank line above) MUST NOT match the function

#### Scenario: Synthetic / external symbols keep zero range

- **WHEN** a symbol is synthetic (no source location) or external (no in-workspace definition)
- **THEN** the `DefRecord` MUST be `[0, 0, 0, 0]` and the symbol MUST be marked `is_external = true`
- **AND** the wire location for the symbol MUST be `null`

## MODIFIED Requirements

### Requirement: Wire location format is `./file_path#start-end`

Every API response field that carries a source location SHALL use the format `./<workspace_relative_path>#<start_line>` (single line) or `./<workspace_relative_path>#<start_line>-<end_line>` (line range). Line numbers in this format are **1-based** — they match the stored `def_range` values, which match what an editor displays.

When no def location applies (synthetic symbols, external symbols without source), the location SHALL be `null`.

The format SHALL include only line numbers; column data SHALL NOT appear in the wire format. Column data remains in the DB on `def_range` for any consumer that needs it.

#### Scenario: Single-line and multi-line locations format correctly

- **WHEN** a class with `def_range = [3, 0, 14, 1]` is materialized
- **THEN** the wire location MUST be `./<path>#3-14`

- **WHEN** a single-line def with `def_range = [42, 8, 42, 32]` is materialized
- **THEN** the wire location MUST be `./<path>#42`

#### Scenario: External symbols have null location

- **WHEN** a symbol with `is_external = true` and `file = 0` is materialized
- **THEN** the wire location MUST be `null`

#### Scenario: First line of a file renders as #1, never #0

- **WHEN** a top-of-file symbol has `def_range = [1, 0, 1, N]`
- **THEN** the wire location MUST be `./<path>#1`
- **AND** the wire location MUST NOT be `./<path>#0`
