## MODIFIED Requirements

### Requirement: the MCP server reports unresolved anchors

The server SHALL expose a `check_anchors` tool that folds every finding's
anchor log, tests each current anchor (a file or directory path) against the
filesystem, and reports anchors that no longer resolve (**broken**) and anchors
that resolve but whose **file content changed** since the anchor's recorded sha
(**drifted**), grouped by finding, so an agent can repair moves and deletions and
re-read drifted directives before a commit. A missing path is broken, not
drifted; a directory anchor, an unreadable path, and a `sha: None` anchor are
live. Because v1 anchors are paths, the check needs only the filesystem, not the
index.

#### Scenario: a moved file surfaces as an unresolved anchor

- **GIVEN** a directive anchored to a file that has since been renamed
- **WHEN** `check_anchors` is called
- **THEN** the report lists that finding and its unresolved anchor path in the
  broken bucket

#### Scenario: an edited file surfaces as a drifted anchor

- **GIVEN** a directive anchored to a file whose content changed since attach
- **WHEN** `check_anchors` is called
- **THEN** the report lists that finding and its anchor path in the drifted
  bucket, not the broken bucket

### Requirement: the MCP server records anchor events

The server SHALL allow an agent to record anchor events for a finding —
`attach`, `rename`, and `detach` — which are appended to that finding's
`<id>.anchor.jsonl`. An `attach` for a path already in the set is the liveness
signal (it confirms the directive applied to current work); an `attach` for a new
path also extends the anchor set. There is no separate confirmation event. When
the attached path resolves to a **file** under the workspace, the server SHALL
compute its content sha at the boundary and record it on the `attach` event, so
later drift can be detected; a directory or unreadable path records no sha.

#### Scenario: re-attaching an anchor updates liveness

- **WHEN** an `attach` event is recorded for a path already in a finding's anchor
  set
- **THEN** that finding's per-anchor recency advances and its relevancy reflects
  the new attach on the next fold

#### Scenario: attaching a file records its content sha

- **WHEN** an `attach` event is recorded for a file path under the workspace
- **THEN** the appended event carries the file's content sha
- **AND** a later edit to that file makes the anchor drift

#### Scenario: detaching removes an anchor

- **WHEN** a `detach` event is recorded for a finding's anchored path
- **THEN** the folded anchor set no longer contains that path

### Requirement: the MCP server exposes path-anchored directive retrieval

The server SHALL expose a `find_directives` tool that, given changed file and/or
directory paths and an optional natural-language query, returns directive and
guide findings ranked by anchor match and recency-weighted liveness fused with
optional body-vector proximity. Superseded and tombstoned findings are excluded.
Each hit SHALL carry a read-time `stale` flag (a cited code-graph node no longer
resolves) and a read-time `drifted` flag (a file the finding is anchored to
changed content since it was anchored), so an agent can re-read a directive whose
ground truth moved before relying on it. The tool degrades to anchor-only ranking
when the embedder is cold.

#### Scenario: a hit whose anchored file changed is flagged drifted

- **GIVEN** a directive anchored to a file whose content changed since attach
- **WHEN** `find_directives` returns that directive for a changed path
- **THEN** the hit carries `drifted: true`

#### Scenario: directive retrieval degrades without the embedder

- **WHEN** `find_directives` is called while the embedder is still warming up
- **THEN** it returns anchor-ranked directives rather than erroring
