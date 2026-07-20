## MODIFIED Requirements

### Requirement: a system-prompt fragment drives finding accumulation

The change SHALL ship a system-prompt fragment instructing an agent to search existing findings before re-investigating and to store a finding at a stable conclusion. The fragment SHALL be installable alongside the MCP server so findings accumulate as a byproduct of normal agent work, independent of the orchestrator in use.

The fragment SHALL additionally drive the directive workflow, orchestrator-independent: before working on an area, recall the directives and guides anchored to the file(s)/dir(s) about to change; before committing, run `check_anchors` and repair moves/deletions, pull directives for the changed files/dirs and warn on `polarity:dont` violations, re-attach the ones that applied, and distill new directions/corrections into directives anchored to the changed files/dirs (superseding when one changed). The squeeze SHALL source the user's directions and the session's touched files from the existing `conversation-history-store` (`collector.db`, branch-filtered) and the `transcript_path` it records — it introduces no new capture mechanism — while anchoring directives to the staged diff's files. The fragment SHALL state the routing rule (small universal rule → CLAUDE.md-style always-on; team-shareable context-specific → directive; machine-local personal style → personal note) and the redaction rule (never write credentials, machine-local paths, or private project/customer identifiers into a committed directive).

#### Scenario: the fragment is available on install

- **WHEN** the MCP server's knowledge layer is installed
- **THEN** the system-prompt fragment is provided as an installable asset
- **AND** it directs the agent to both query and store findings

#### Scenario: the fragment drives recall and the before-commit ritual

- **WHEN** the fragment is read by any orchestrator's agent
- **THEN** it directs recalling directives by file/dir before work
- **AND** it directs the before-commit check → pull → re-attach → squeeze ritual
  with the routing and redaction rules

## ADDED Requirements

### Requirement: the MCP server exposes path-anchored directive retrieval

The server SHALL expose a `find_directives` tool that, given a set of file and/or
directory paths, returns findings tagged `directive` or `guide` that are relevant
to those paths. Ranking SHALL RRF-fuse two ranked lists — a structural leg
(exact-path anchor match plus ancestor-directory subtree anchor match) and a
semantic leg (body-vector proximity) — and SHALL then reweight the fused score by
a recency-weighted anchor liveness (more recently and more frequently re-attached
ranks higher), so a decayed anchor loses rank but is not dropped. When the
embedder or index is not ready (the semantic leg would return `EMBEDDER_STARTING`,
`INDEX_UNAVAILABLE`, or `EMPTY_SNAPSHOT`), the tool SHALL degrade to the
structural leg alone and still return anchor-matched results rather than
erroring, because anchors are committed and resolvable without an index. As
retrieval, the tool SHALL respect the supersede/tombstone rules — prefer the
latest finding in a supersede chain and exclude tombstoned findings — so a
corrected or retired directive does not resurface, and SHALL carry the same
read-time `stale` flag as other retrieval (a directive whose code-graph
`parent_ids` no longer resolve is returned marked stale, not omitted). Note this
is distinct from anchor resolution: `stale` reflects `parent_ids` (the evidence),
while `check_anchors` reports unresolved anchors (where it applies). The tool
SHALL NOT fabricate results: when nothing is relevant it returns an empty set.

#### Scenario: a file query returns directives anchored to it and its dir

- **GIVEN** a directive anchored to `crates/kenn-mcp/src/server.rs` and another
  anchored to the directory `crates/kenn-mcp/`
- **WHEN** `find_directives` is called with path `crates/kenn-mcp/src/server.rs`
- **THEN** both directives are returned

#### Scenario: liveness orders the results

- **GIVEN** two relevant directives, one re-attached recently and often and one
  attached once long ago
- **WHEN** `find_directives` is called for their anchored path
- **THEN** the recently-and-frequently-re-attached directive ranks ahead

#### Scenario: retrieval degrades to anchors when the index is cold

- **GIVEN** the embedder/index is not ready (the semantic leg would return a
  `-32002` service-unavailable error)
- **WHEN** `find_directives` is called with a path an existing directive is
  anchored to
- **THEN** the anchor-matched directive is returned from the structural leg alone
- **AND** the call does not error

#### Scenario: a directive over removed evidence is marked stale, not dropped

- **GIVEN** a directive whose `parent_ids` cite a code-graph node since removed,
  still anchored to a current file
- **WHEN** `find_directives` is called with that file's path
- **THEN** the directive is returned with a `stale` flag, not omitted

#### Scenario: a superseded directive does not resurface

- **GIVEN** a directive anchored to a file and a newer finding that supersedes it
- **WHEN** `find_directives` is called with that file's path
- **THEN** only the superseding finding is returned, not the original
- **AND** a tombstoned directive for that path is not returned at all

#### Scenario: no relevant directive returns empty

- **WHEN** `find_directives` is called with a path no directive is anchored to
  or semantically near
- **THEN** the result is empty

### Requirement: the MCP server reports unresolved anchors

The server SHALL expose a `check_anchors` tool that folds every finding's
anchor log, tests each current anchor (a file or directory path) against the
filesystem, and reports the anchors that no longer resolve, grouped by finding,
so an agent can repair moves and deletions before a commit. Because v1 anchors
are paths, the check needs only the filesystem, not the index.

#### Scenario: a moved file surfaces as an unresolved anchor

- **GIVEN** a directive anchored to a file that has since been renamed
- **WHEN** `check_anchors` is called
- **THEN** the report lists that finding and its unresolved anchor path

### Requirement: the MCP server records anchor events

The server SHALL allow an agent to record anchor events for a finding —
`attach`, `rename`, and `detach` — which are appended to that finding's
`<id>.anchor.jsonl`. An `attach` for a path already in the set is the liveness
signal (it confirms the directive applied to current work); an `attach` for a new
path also extends the anchor set. There is no separate confirmation event.

#### Scenario: re-attaching an anchor updates liveness

- **WHEN** an `attach` event is recorded for a path already in a finding's anchor
  set
- **THEN** that finding's per-anchor recency advances and its relevancy reflects
  the new attach on the next fold

#### Scenario: detaching removes an anchor

- **WHEN** a `detach` event is recorded for a finding's anchored path
- **THEN** the folded anchor set no longer contains that path

## MODIFIED Requirements

### Requirement: the MCP server exposes finding writes with provenance

The server SHALL expose `store_finding`, accepting `text`, `parent_ids`, `tags`,
and an optional `anchors` list (file or directory paths), and returning the new
finding's id together with any semantically near existing findings. When
`anchors` are supplied, the server SHALL record an initial `attach` event for
each in the new finding's `<id>.anchor.jsonl`, so a directive is created and
anchored in one call. It SHALL expose `merge_findings`, which synthesizes a new
finding from given finding ids and records those ids as parents.

Both SHALL validate their id inputs before writing. A `fnd_…` id that names no
existing finding SHALL fail the call with `INVALID_INPUT`, and the error SHALL
list **every** unresolved id, not only the first, so the caller corrects them in
one round-trip. `merge_findings` inputs are findings, so every input id is
checked. `store_finding`'s `parent_ids` mix finding ids and code-graph node ids;
only the `fnd_…` ones are checked — a code-node reference is best-effort
provenance whose later resolvability is reported by finding staleness, not
enforced at write time.

#### Scenario: store_finding returns id and near-duplicates

- **WHEN** `store_finding` is called and a semantically similar finding already
  exists
- **THEN** the response contains the new finding's id
- **AND** the response lists the similar prior finding

#### Scenario: store_finding anchors the new finding in one call

- **WHEN** `store_finding` is called with an `anchors` list
- **THEN** the new finding's `<id>.anchor.jsonl` records an `attach` for each
  anchor

#### Scenario: merge_findings records its inputs as parents

- **WHEN** `merge_findings` is called with two finding ids
- **THEN** a new finding is created whose `parent_ids` include both inputs

#### Scenario: unknown finding inputs are rejected, all at once

- **WHEN** `store_finding` or `merge_findings` is called with two `fnd_…` ids
  that name no existing finding
- **THEN** the response is an `INVALID_INPUT` error
- **AND** the error message names both unresolved ids
