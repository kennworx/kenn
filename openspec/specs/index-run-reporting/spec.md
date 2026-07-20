# index-run-reporting Specification

## Purpose
TBD - created by archiving change index-status-error-reporting. Update Purpose after archive.
## Requirements
### Requirement: kenn index prints a per-language summary on degraded runs

When the aggregate run status is not `Success`, `kenn index` SHALL print one
stderr line per language whose reports contain any non-`Success` status,
naming the language, its worst status, and the first `failed_projects`
entry (with a `+N more` suffix counting further attributions, including
each report's structured `failed_overflow`). Reports SHALL be grouped by
the language they carry (`RunReport.language`, stated by the driver at
construction), so a language's branded per-unit reports (`rust-analyzer`)
and its language-id failure reports (`rust`) collapse into one line.
Languages whose reports are all `Success` SHALL NOT be mentioned, and a
fully successful run SHALL print no summary lines at all.

#### Scenario: a failed Rust unit is named at index time

- **WHEN** rust-analyzer exits non-zero for the only Cargo workspace while
  the markdown producer succeeds
- **THEN** `kenn index` prints a stderr line naming `rust`, its `failed`
  status, and the rust-analyzer failure message
- **AND** prints no line for `markdown`

#### Scenario: a clean run stays quiet

- **WHEN** every producer reports `Success`
- **THEN** no per-language summary lines are printed

### Requirement: producer warnings are shown at index time and in status

`kenn index` SHALL print one stderr line per language whose reports carry
producer warnings — independent of run status, because the warnings exist
precisely for degradations that keep the unit `Success` (e.g. stale
index-store units kept on a trusted read). `kenn status` SHALL show the
persisted warnings with their true count (retained entries plus
structured overflow, rendered `+N more`).

#### Scenario: a trusted-store staleness notice reaches the user

- **WHEN** the Swift sidecar keeps mtime-stale units on a `--skip-build`
  read and emits a warning frame saying so, and the unit's status is
  `Success`
- **THEN** `kenn index` prints `warning: swift: <notice>` and
  `kenn status` lists the notice under `warnings (N)`

### Requirement: an enabled language that indexed zero files triggers a warning

`kenn index` SHALL print a stderr warning when a language's reports contain
at least one non-`Success` status and the sum of indexed files across its
reports is zero, naming the language and its claimed extensions and stating
that files with those extensions are absent from the snapshot (claimed
extensions are skipped by the text fallback). A language whose reports are
all `Success` SHALL NOT trigger the warning even at zero files — JSONL
producers report once per run even when the workspace has no sources of
that language, and an intentionally empty language is not a coverage hole.

#### Scenario: a failed semantic pass surfaces the coverage hole

- **WHEN** `[language.rust]` is enabled, one Cargo unit is discovered, and
  rust-analyzer fails so zero files are ingested
- **THEN** `kenn index` warns that `rust` indexed 0 files and `.rs` files
  are not covered by the text fallback

#### Scenario: an enabled language with no sources stays quiet

- **WHEN** `[language.swift]` is enabled, the workspace contains no
  `Package.swift` or Xcode project, and the sidecar reports `Success` with
  zero files
- **THEN** no zero-files warning is printed for `swift`

#### Scenario: a language's mixed-name reports do not split the rollup

- **WHEN** rust-analyzer's per-unit report (`rust-analyzer`, `Success`,
  many files) coexists with a `rust`-named finalize-failure report
  (`Failed`, 0 files) in one run
- **THEN** both roll up under `rust`: one summary line is printed
- **AND** no false zero-files warning fires (the language indexed files)

