# findings-mcp Specification

## Purpose
TBD - created by archiving change findings-mcp-layer. Update Purpose after archive.
## Requirements
### Requirement: the MCP server exposes unified search over code and findings

The server SHALL expose a `semantic_search` tool that ranks results by BM25 and that can be scoped to code, to findings, or to both. It SHALL also expose `get_source` over the code graph, and `get_finding` and `search_findings` over the findings store.

Vector / semantic ranking is **deferred**: it depends on the `embedding-producer` change. Until that lands, `semantic_search` is lexical (BM25) — at which point this requirement becomes hybrid BM25 + vector. Code-graph node, caller, and callee reads are already served by the existing `get_symbol` / `list_callers` / `list_callees` tools and SHALL NOT be re-exposed under new names.

#### Scenario: a query spanning code and findings returns both

- **WHEN** `semantic_search` is called with a query and a scope covering code and findings
- **THEN** the ranked result includes matching code symbols and matching findings

#### Scenario: a finding is retrievable by id

- **WHEN** `get_finding` is called with a known finding id
- **THEN** the server returns that finding's text, tags, and `parent_ids`

### Requirement: the MCP server exposes finding writes with provenance

The server SHALL expose `store_finding`, accepting `text`, `parent_ids`, `tags`,
and an optional `anchors` list (file or directory paths), and returning the new
finding's id together with any semantically near existing findings. When
`anchors` are supplied, the server SHALL record an initial `attach` event for
each in the new finding's `<id>.anchor.jsonl`, so a directive is created and
anchored in one call. The near-duplicate probe is **advisory**: it pre-embeds the
text to find similar prior findings, but when the embedder is cold the server
SHALL still write the finding and return its id with an empty `similar` list,
rather than failing the write — matching `find_directives`' non-blocking degrade,
so the first write against a freshly-indexed repo (whose embeddings are not yet
built) succeeds. It SHALL expose `merge_findings`, which synthesizes a new
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

#### Scenario: store_finding succeeds while the embedder is cold

- **WHEN** `store_finding` is called before the embedder has warmed (a freshly
  indexed repo with no embeddings yet)
- **THEN** the finding is written and its id is returned
- **AND** the `similar` list is empty rather than the call failing

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

### Requirement: the MCP server exposes derivation-DAG traversal

The server SHALL expose `find_predecessors` and `find_successors`, walking the `parent_ids` edges of the unified ID space so a caller can trace a finding back to the code or earlier findings it was derived from.

The start id SHALL be validated: a `fnd_…` id that names no existing finding SHALL fail the call with `INVALID_INPUT` rather than return an empty walk. A code-node start id is accepted without a code-graph lookup — a code node has no predecessors, and `find_successors` from a refactored-away node must still reach the findings that cite it.

#### Scenario: provenance is walkable to source

- **GIVEN** a finding derived from another finding that cites a code-graph node
- **WHEN** `find_predecessors` is walked transitively from the finding
- **THEN** the walk reaches the originating code-graph node

#### Scenario: an unknown finding start id is rejected

- **WHEN** `find_predecessors` or `find_successors` is called with a `fnd_…` id that names no existing finding
- **THEN** the response is an `INVALID_INPUT` error

### Requirement: the MCP server runs no model and performs no task analysis

The server SHALL expose only primitive capabilities — graph reads, finding reads and writes, DAG traversal. It SHALL NOT host an embedding or language model, and SHALL NOT expose a tool that interprets a task, plans work, or slices work for subagents. Slicing and dispatch are the calling agent's responsibility.

#### Scenario: no planning or slicing tool is offered

- **WHEN** the server's tool list is enumerated
- **THEN** it contains search, graph-read, finding-read, finding-write, and DAG tools
- **AND** it contains no tool that analyzes a task or produces a work plan

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

### Requirement: the subagent-as-extractor pattern is documented

The change SHALL document the subagent-as-extractor dispatch pattern: a main agent orients with search and graph reads, slices the task, fans out subagents that each investigate through the MCP surface and record findings, and synthesizes the returned finding ids. The documentation SHALL state that coordination is through the findings store and returned ids, not ad-hoc file passing.

#### Scenario: the dispatch pattern is described for implementers and agents

- **WHEN** the knowledge-layer documentation is read
- **THEN** it describes the orient → slice → fan-out → record → synthesize flow
- **AND** it states that subagents coordinate via stored findings and returned ids

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

