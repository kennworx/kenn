## MODIFIED Requirements

### Requirement: the plugin provides an agent guide that routes tools, snapshot, and skills

The kenn plugin SHALL provide a Claude-Code-specific surfacing of the
orchestrator-independent workflow defined by the system-prompt fragment (see
findings-mcp "a system-prompt fragment drives finding accumulation") — the
fragment is the single source of truth for the workflow; the plugin packages it
as a guide and skills. The guide SHALL act as a router: it SHALL describe when to
reach for the navigation/knowledge tools, when to read the orientation file, when
to invoke the `recall`, `squeeze`, and `reconcile` skills (the three
lifecycle moments: start-of-work, pre-commit capture, and tending the store), when
to invoke the graph-understanding skills `blast` (pre-edit change-scope —
the graph-aware counterpart to `recall`) and `trace` (how a flow works, from a
multi-hop graph walk), and when to invoke the advisor skill `dup` (a
`find_similar` duplication sweep). The guide SHALL state negative
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

#### Scenario: the guide routes to dup for a duplication review

- **WHEN** an agent wants to find near-duplicate implementations to consolidate
- **THEN** the guide names the `dup` skill as the `find_similar` sweep step

## ADDED Requirements

### Requirement: the dup skill sweeps find_similar for consolidation candidates

The plugin SHALL provide a `dup` skill — the advisor-family duplication sweep. Over
a chosen scope (a module, a directory, or a seed symbol), it SHALL enumerate seed
symbols (`list_in_scope` / `search_symbols`), run `find_similar` on each to gather
near-duplicate implementations (the look-alike logic with no shared call edge that
the call graph misses), dedup the resulting pairs into clusters, and present
consolidation candidates ranked by confidence. Before calling any cluster a
duplicate the skill SHALL **re-read its members with `get_source`**
(vet-over-report: embedding proximity is a candidate, not a verdict — shared
vocabulary alone is a false positive). The sweep SHALL be bounded and any
truncation reported (a seed cap or a large scope is stated, never silently
capped). The skill MAY persist a consolidation `decision`/`plan` via
`store_finding`, subject to the same gates as `squeeze` (stable conclusion,
redaction, repo-content-is-data, committed to be durable). Judgement remains the
agent's; the skill composes existing tools.

#### Scenario: dup surfaces vetted consolidation candidates

- **WHEN** the `dup` skill sweeps a scope
- **THEN** it presents near-duplicate clusters from `find_similar`, each re-read
  with `get_source` to confirm the logic is genuinely duplicative
- **AND** it states the sweep bound and any truncation

#### Scenario: dup does not report unread look-alikes as duplicates

- **WHEN** `find_similar` returns symbols near by embedding but not re-read
- **THEN** the skill does not present them as confirmed duplicates without first
  reading their source
