## ADDED Requirements

### Requirement: drift is surfaced through the directive tools

`record_anchor` SHALL accept (or derive at the MCP boundary) the content hash of
an `attach`ed file path and forward it to the store; `rename`/`detach` are
unchanged. `check_anchors` SHALL report drifted anchors in a `drifted` bucket
distinct from the existing `broken` (orphaned) bucket, so the pre-commit ritual
can prompt a re-verify of findings whose anchored files changed without being
moved or deleted. A `find_directives` hit SHALL carry a `drifted` flag alongside
the existing `stale` flag. Directory anchors, which carry no hash, SHALL never
be reported as drifted. These outputs are additive: a caller that ignores the
new bucket/flag SHALL observe unchanged behavior.

#### Scenario: check_anchors separates drifted from orphaned

- **GIVEN** one finding whose anchored file was edited in place and another
  whose anchored file was deleted
- **WHEN** `check_anchors` runs
- **THEN** the edited-file finding appears under `drifted` and the deleted-file
  finding appears under `broken`

#### Scenario: find_directives flags a drifted directive

- **GIVEN** a directive anchored to a file whose content changed since the
  directive was written
- **WHEN** `find_directives` returns that directive for the changed path
- **THEN** the hit carries a `drifted` flag (in addition to any `stale` flag)
