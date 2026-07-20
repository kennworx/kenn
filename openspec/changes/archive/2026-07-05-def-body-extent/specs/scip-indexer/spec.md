## ADDED Requirements

### Requirement: SCIP definition enclosing_range populates the def body extent

The SCIP transform SHALL read `Occurrence.enclosing_range` for each
**definition** occurrence and, when present, map it onto the `DefRecord` body
extent (`body_start_line` / `body_end_line`, 1-based). Like `range`,
`enclosing_range` is 0-based on both axes and comes in the 3-int (single-line)
or 4-int (multi-line) shape; the transform SHALL add `+1` to the line values,
matching the name-range convention.

When `enclosing_range` is empty — an older rust-analyzer that does not emit it,
or a producer (scip-go / scip-python) that omits it for a given occurrence — the
body extent SHALL be `0` (absent). The zero-range synthetic placeholder emitted
for a symbol with no definition occurrence SHALL also carry a `0` body extent.

Capturing the body extent MUST NOT change the def's name range, nor the
`DocumentDefIndex` used for edge FROM-attribution (which already reads
`enclosing_range` independently).

#### Scenario: a definition with enclosing_range gets a body span

- **WHEN** a definition occurrence reports `range = [45, 0, 45, 11]` and
  `enclosing_range = [41, 0, 236, 1]` (0-based)
- **THEN** the resulting `DefRecord` MUST have `start_line = 46` (name, `+1`)
- **AND** `body_start_line = 42`, `body_end_line = 237` (enclosing, `+1`)

#### Scenario: a definition without enclosing_range gets a zero body span

- **WHEN** a definition occurrence reports a `range` but an empty
  `enclosing_range` (e.g. a pre-Dec-2025 rust-analyzer)
- **THEN** the resulting `DefRecord` MUST have `body_start_line = 0` and
  `body_end_line = 0`
- **AND** `get_source` for that symbol falls back to the declaration line

### Requirement: a too-old rust-analyzer is surfaced, not silently degraded

The indexer SHALL emit a one-time warning when a completed Rust index yields
**zero** definition body extents, identifying the resolved rust-analyzer as too
old for full-item `get_source` and recommending an upgrade (Homebrew
`rust-analyzer` or `rustup update`). `rust-analyzer` emits
`Occurrence.enclosing_range` on definitions only from ~Dec-2025 onward, and the
rustup-bundled build lags the standalone release. Indexing SHALL otherwise
succeed; Rust `get_source` degrades to declaration lines.

#### Scenario: old rust-analyzer triggers an upgrade warning

- **WHEN** a Rust index completes and no definition carried an `enclosing_range`
- **THEN** the run SHALL log a warning naming the too-old rust-analyzer and the
  upgrade path
- **AND** the index SHALL still publish successfully
