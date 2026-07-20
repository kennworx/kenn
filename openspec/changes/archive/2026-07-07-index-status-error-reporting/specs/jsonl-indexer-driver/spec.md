## MODIFIED Requirements

### Requirement: One run report per invocation

For a JSONL indexer invocation covering N `.sln`s, the pipeline SHALL
produce exactly one `RunReport`. Per-project failure attribution
SHALL be preserved by populating the report's `failed_projects` from
`ErrorFrame{severity: error}` frames in the stream, formatted from the
frame's `source`, `path` (when present), and `message`. The number of
retained entries SHALL be bounded (first 32); attributions past the cap
SHALL be recorded as a structured `failed_overflow` count on the report —
never as a synthetic list entry — so counting consumers see the true
total (`failed_projects.len() + failed_overflow`) and display surfaces
render the overflow as a `+N more` suffix.

Severity SHALL be validated at parse time, case-insensitively: `error`
(and any unrecognized severity — fail loud, never silently drop
attribution) degrades the report; `warning` does not. A stream containing
at least one such error frame SHALL degrade the report's status to
`Partial` unless the report is already `Failed`.

Warning-severity frames SHALL be recorded on the report's `warnings`
(bounded like `failed_projects`, with a structured `warnings_overflow`),
status-neutral — producers emit them for degradations that keep the run
useful (e.g. stale index-store units kept on a trusted read), and a
warning that dies in a counter silences a diagnostic the producer
promised the user.

When the indexer process exits non-zero, the failure message recorded in
`failed_projects` SHALL name the report's producer (`indexer_name`, e.g.
`kenn-ts`) — stable even under runner-form command configs such as
`["dotnet", "kenn-dotnet.dll"]`.

The report's unit identifier MAY be a synthetic value such as the
workspace root path or the indexer language id; it MUST NOT be a
single `.sln` path because the invocation covers multiple.

#### Scenario: Per-project load failures surface with paths

- **GIVEN** a workspace where two of three `.sln`s fail to load
- **WHEN** the indexer emits `ErrorFrame{severity:"error",
  source:"indexer", path:"<sln1>"}` and another for `<sln3>` (per-entry
  load failures carry the entry path; msbuild workspace diagnostics are
  message-only and surface as pathless `msbuild: <message>` attributions)
- **THEN** the resulting RunReport's `failed_projects` MUST list
  both `<sln1>` and `<sln3>`
- **AND** the report status MUST be `Partial`
- **AND** `kenn status` MUST show those two `.sln`s as failed

#### Scenario: error frames degrade a clean exit to Partial

- **WHEN** a JSONL indexer emits one `ErrorFrame{severity:"error"}` and
  then exits 0
- **THEN** the unit's report status MUST be `Partial`, not `Success`

#### Scenario: warnings do not degrade status but are surfaced

- **WHEN** a JSONL indexer emits only `ErrorFrame{severity:"warning"}`
  frames and exits 0
- **THEN** the unit's report status MUST remain `Success`
- **AND** the warnings MUST be recorded on the report and shown by
  `kenn status`

#### Scenario: overflow is a count, not a list entry

- **WHEN** a stream carries 40 `severity:"error"` frames
- **THEN** the report retains 32 attributions and `failed_overflow` = 8
- **AND** no `failed_projects` entry is a synthetic marker — `kenn status`
  reports the true count (40) and renders `+8 more` at display time only

#### Scenario: a non-zero exit names the right producer

- **WHEN** `kenn-ts` exits non-zero
- **THEN** the `failed_projects` message MUST contain `kenn-ts`, not the
  name of a different producer, even when the configured command is a
  runner form
