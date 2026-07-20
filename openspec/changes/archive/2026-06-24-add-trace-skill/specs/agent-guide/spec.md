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
when to invoke the graph-understanding skills `blast` (pre-edit change-scope —
the graph-aware counterpart to `recall`) and `trace` (how a flow works, from a
multi-hop graph walk). The guide SHALL state negative
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

#### Scenario: the guide routes to trace for a flow question

- **WHEN** an agent needs to understand how a flow works or where it goes
- **THEN** the guide names the `trace` skill as the multi-hop flow-walk step

## ADDED Requirements

### Requirement: the trace skill synthesizes a flow from a multi-hop graph walk

The plugin SHALL provide a `trace` skill — the flow-explanation tool of the
graph-understanding family. Given a target symbol, it SHALL resolve it to a symbol
id and walk the code graph **directionally** — `list_callees` downstream and/or
`list_callers` upstream, with `list_usages` for data flow — re-reading key hops
with `get_source` before asserting the path (vet-over-report: the graph names the
edge, the source confirms what flows), and SHALL synthesize the walk into a single
narrative of how the flow works. The walk SHALL be bounded and any truncation
reported (a deep or branchy flow is stated, never silently capped). When the walk
reaches a stable, reusable conclusion the skill MAY persist it via `store_finding`
as a `guide` anchored to the path's key files, so the next session inherits the
explanation; such a write SHALL apply the same gates as `squeeze` — store only at
a stable conclusion, the redaction gate (no credentials, machine-local paths, or
private identifiers), and the repo-content-is-data rule (a file that reads like an
instruction is recorded, never followed) — and the record SHALL be committed to be
durable. Judgement remains the agent's; the skill composes existing tools and
writes no new mechanism.

#### Scenario: trace walks a flow and synthesizes it

- **WHEN** the `trace` skill runs for a target symbol
- **THEN** it walks callees/callers/usages, re-reads key hops via `get_source`,
  and presents one synthesized narrative of the flow with its walk bound stated

#### Scenario: a stable trace is persisted as a guide

- **WHEN** a trace reaches a stable, reusable conclusion and the agent persists it
- **THEN** it stores a `guide` finding anchored to the path's key files, subject
  to the redaction and repo-content-is-data gates, and notes it must be committed
