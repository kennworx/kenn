## MODIFIED Requirements

### Requirement: the plugin provides an agent guide that routes tools, snapshot, and skills

The kenn plugin SHALL provide a Claude-Code-specific surfacing of the
orchestrator-independent workflow defined by the system-prompt fragment (see
findings-mcp "a system-prompt fragment drives finding accumulation") — the
fragment is the single source of truth for the workflow; the plugin packages it
as a guide and skills. The guide SHALL act as a router: it SHALL describe when to
reach for the navigation/knowledge tools, when to read the orientation file, when
to invoke the `recall`, `squeeze`, and `reconcile` skills (the three
lifecycle moments: start-of-work, pre-commit capture, and tending the store), and
when to invoke the `blast` skill (pre-edit change-scope — the graph-aware
counterpart to `recall`). The guide SHALL state negative
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

#### Scenario: the guide routes to blast before an edit

- **WHEN** an agent is about to change a symbol and needs its change scope
- **THEN** the guide names the `blast` skill as the pre-edit change-scope step

## ADDED Requirements

### Requirement: the blast skill reports pre-edit change scope

The plugin SHALL provide a `blast` skill — the pre-edit blast-radius tool and the
graph-aware counterpart to `recall`. Given a target symbol (or file) the agent is
about to change, it SHALL resolve the target to a symbol id and walk the code
graph transitively — incoming `list_callers`, `list_usages`, `list_implementers`,
and `list_overrides` — to compute the **change surface** (the affected symbols and
their files), then run `find_directives` over those files to surface the **rules
that govern them**. It SHALL present the change surface grouped by relation and the
governing directives together (change surface ∪ governing memory), heeding
`polarity:dont` rules and flagging `stale`/`drifted` directives. The walk SHALL be
bounded and the skill SHALL report any truncation explicitly (a hop limit or a
large fan-in is stated, never silently dropped). The skill is read-only — it
orients before an edit and SHALL NOT write findings; judgement remains the
agent's.

#### Scenario: blast reports the change surface and its governing rules

- **WHEN** the `blast` skill runs for a symbol about to be changed
- **THEN** it presents the transitive callers/usages/implementers as the change
  surface, grouped by relation
- **AND** it presents the directives anchored to the touched files, with
  `polarity:dont` rules and any `stale`/`drifted` flags called out

#### Scenario: blast reports truncation rather than capping silently

- **WHEN** the change surface exceeds the skill's walk bound
- **THEN** the skill states that the surface was truncated and where, rather than
  presenting a silently capped set as complete
