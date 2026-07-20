## ADDED Requirements

### Requirement: the plugin provides an agent guide that routes tools, snapshot, and skills

The kenn plugin SHALL provide a Claude-Code-specific surfacing of the
orchestrator-independent workflow defined by the system-prompt fragment (see
findings-mcp "a system-prompt fragment drives finding accumulation") — the
fragment is the single source of truth for the workflow; the plugin packages it
as a guide and skills. The guide SHALL act as a router: it SHALL describe when to
reach for the navigation/knowledge tools, when to read the orientation file, and
when to invoke the `squeeze` and `recall` skills. The guide SHALL state negative
triggers (when not to use kenn) and SHALL spell out the missing-file fallback for
orientation. The guide SHALL NOT place reasoning in kenn: it instructs the agent
how to access and combine kenn's knowledge, while classification, distillation,
and judgement remain the agent's (consistent with findings-mcp "the MCP server
runs no model and performs no task analysis").

#### Scenario: the guide routes by moment of work

- **WHEN** an agent needs orientation, navigation, or to capture/recall steering
- **THEN** the guide names the snapshot file, the tools, or the skill to use for
  that moment, with the routing rule between CLAUDE.md, kenn directives, and
  personal notes

### Requirement: orientation is a snapshot file with a tool fallback

Orientation SHALL be delivered as a run-local `overview.md` — written into the
active index run directory (`<derived_root>/runs/{id}/`, reached via `live`), not
a separate snapshots directory and not an MCP resource. The agent reads it
directly. When the file is absent (no current run, e.g. indexing not finished),
the guide SHALL direct the agent to call the status/overview tool. The absence of the file is the readiness signal. Because a
present file can still be stale (the watcher may lag), `overview.md` SHALL carry
the snapshot's `indexed_at` and the guide SHALL tell the agent to treat a
far-past `indexed_at` as a prompt to check `get_index_status` / reindex rather
than trust presence alone.

#### Scenario: present snapshot is read directly

- **GIVEN** a current `overview.md` exists with a recent `indexed_at`
- **WHEN** the agent orients
- **THEN** it reads the file directly

#### Scenario: a stale-but-present snapshot prompts a freshness check

- **GIVEN** an `overview.md` exists whose `indexed_at` is far in the past
- **WHEN** the agent orients
- **THEN** the guide directs it to verify freshness via `get_index_status`
  rather than trust the file

#### Scenario: absent snapshot falls back to a tool

- **GIVEN** no current `overview.md` exists
- **WHEN** the agent orients
- **THEN** the guide directs it to call `get_index_status` /
  `get_workspace_overview`

### Requirement: the recall skill surfaces relevant directives by file/dir

The plugin SHALL provide a `recall` skill that packages, for Claude Code, the
fragment's start-of-work recall step (the fragment is the source of truth). As
the start-of-work counterpart to `squeeze`, when the agent begins work on an area
it pulls the directives and
guides relevant to the file(s)/dir(s) it is about to touch via `find_directives`,
and presents them so the agent starts informed: directives (the rules) grouped by
polarity, and guides (the context) distinctly. It SHALL NOT fabricate: if
retrieval is empty it says so rather than inventing a directive.

#### Scenario: recall presents rules and context distinctly

- **WHEN** the `recall` skill runs for a file the agent is about to work on
- **THEN** it presents the `directive` results grouped by polarity and the
  `guide` results as context
- **AND** if none are returned it reports that none were found

### Requirement: the squeeze skill captures directives before commit

The plugin SHALL provide a `squeeze` skill that packages, for Claude Code, the
fragment's before-commit ritual (the fragment is the source of truth), driven by
guidance rather than a git hook: (1) `check_anchors` and repair moves /
deletions from the diff; (2) PULL `find_directives` for the changed files/dirs,
judge which actually apply, and warn when the diff violates a directive — the
check is seeded by `polarity:dont` directives (the do-not rules) and is the
agent's judgement, not a kenn computation, and `guide` findings are context only,
never violation-checked; (3) record an `attach` event for each confirmed-relevant
directive;
(4) distill this session's directions and corrections — sourced from the existing
`conversation-history-store` (`collector.db` prompts + branch-filtered touched
files, and `transcript_path` for depth), not a new capture path — into new
directives, favoring corrections and recurrence over praise, anchored to the
changed files/dirs, superseding when a directive changed, and applying the
constitution / case-law / personal-note routing rule, asking the user when the
intent or home is unclear. Before writing a committed directive the skill SHALL
apply a redaction gate: it SHALL NOT write credentials, machine-local absolute
paths, or private project/customer identifiers into a committed directive,
routing anything unshareable to a machine-local personal note instead.

#### Scenario: the ritual catches a violation and captures a new directive

- **GIVEN** a staged diff that contradicts an existing directive anchored to a
  changed file
- **WHEN** the `squeeze` skill runs
- **THEN** it warns that the diff violates the directive
- **AND** it stores any new session directions as directives anchored to the
  changed files, recording `attach` events for the directives that applied

#### Scenario: anchors are repaired before commit

- **GIVEN** the diff renamed a file that a directive is anchored to
- **WHEN** the `squeeze` skill runs `check_anchors`
- **THEN** the unresolved anchor is reported and repaired with a `rename` event

#### Scenario: unshareable steering is not committed

- **GIVEN** a session direction that includes a credential or a private
  project/customer identifier
- **WHEN** the `squeeze` skill distills directives
- **THEN** it does not write that secret/identifier into a committed directive
- **AND** it routes the unshareable content to a machine-local personal note
