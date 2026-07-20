## ADDED Requirements

### Requirement: defs carry an enclosing-item body extent distinct from the name span

The `defs` table SHALL carry two additional columns, `body_start_line` and
`body_end_line`, holding the **1-based** line span of the whole enclosing item
(a function/method/type/impl body, including its outer doc comment and
attributes) that the definition names. These are **lines only** — no columns —
because the sole consumer, `get_source`, slices whole lines.

The body extent is distinct from the name span
(`start_line/start_col/end_line/end_col`), which continues to hold the
identifier range used by `find_at_location`, edge anchoring, and location
rendering. The body extent MUST NOT be derived by overloading the name span's
`end_line/end_col`.

A definition with no producer-supplied extent — an older rust-analyzer that
emits no `enclosing_range`, a synthetic/external symbol, or a producer that does
not yet emit a body range — SHALL store `body_start_line = 0` and
`body_end_line = 0` (the "absent" sentinel). The columns default to `0`.

Because the extent excludes trivia other than doc comments, `body_start_line`
MAY be **less than** `start_line` (the doc comment / attribute sits above the
name line).

#### Scenario: A multi-line function stores its whole-item span

- **WHEN** a definition's name is on file line 46 and its enclosing item spans
  lines 42–237 (a leading `#[…]` attribute through the closing brace)
- **THEN** the stored `DefRecord` MUST have `start_line = 46` (the name)
- **AND** `body_start_line = 42`, `body_end_line = 237`

#### Scenario: A definition with no producer extent stores zero body span

- **WHEN** an indexer supplies a definition with a name range but no enclosing /
  body range
- **THEN** the stored `DefRecord` MUST have `body_start_line = 0` and
  `body_end_line = 0`

### Requirement: get_source returns the enclosing item when an extent is present

`get_source` SHALL slice the stored body extent when it is present — defined as
`body_start_line >= 1` and `body_end_line >= body_start_line` — returning the
whole item (doc comment / attributes through the closing brace) and reporting
`start_line`/`end_line` equal to the body span it sliced.

When the body extent is absent (`body_start_line = 0`), `get_source` SHALL fall
back to the **name span** (`start_line … end_line`) — the declaration line for a
def whose name range is a single line. `get_source` SHALL NOT parse source to
synthesize an extent.

#### Scenario: full item returned when the extent is stored

- **WHEN** `get_source` is called for a symbol whose def has
  `body_start_line = 42, body_end_line = 237`
- **THEN** the response `start_line` MUST be 42 and `end_line` MUST be 237
- **AND** `text` MUST be lines 42–237 of the file

#### Scenario: declaration line returned when no extent is stored

- **WHEN** `get_source` is called for a symbol whose def has
  `body_start_line = 0` and a single-line name span at line 46
- **THEN** the response MUST return line 46 (the declaration line), unchanged
  from the pre-extent behavior
