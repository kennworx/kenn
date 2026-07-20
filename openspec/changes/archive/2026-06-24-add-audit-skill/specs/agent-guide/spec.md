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
multi-hop graph walk), and when to invoke the advisor skills `dup` (a
`find_similar` duplication sweep) and `audit` (a deep, graph-backed codebase audit
with reject-memory). The guide SHALL state negative
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

#### Scenario: the guide routes to audit for a deep review

- **WHEN** an agent wants a codebase-wide audit (duplication, dead code,
  god-modules) rather than a single-axis sweep
- **THEN** the guide names the `audit` skill as the graph-backed pipeline

## ADDED Requirements

### Requirement: the audit skill runs a graph-backed codebase audit with reject-memory

The plugin SHALL provide an `audit` skill — the advisor-family deep audit, the
`improve` pipeline (recon → audit → vet → prioritize → plan) with the mechanical
legs querying the code graph. Over a chosen scope it SHALL: scope via
`get_workspace_overview` and the orientation file; run graph-backed legs —
**duplication** via `find_similar` (the `dup` sweep), **dead code** via symbols
with no inbound `find_usages`, and **god-modules** via `list_callers` fan-in and
module size; **vet** every candidate by re-reading its source with `get_source`
before it enters the report (vet-over-report — a graph signal is a candidate, not
a verdict); **prioritize** by impact × confidence; and end in a concrete plan.
Sweeps SHALL be bounded with truncation reported. Dead-code candidates SHALL carry
the standard caveats (entry points, dependency-injection/reflection, framework
hooks, serialization, test-only usage) because no inbound edge does not prove
deadness. The skill SHALL use **reject-memory**: before flagging it SHALL
`search_findings` for prior `reject`-tagged findings so it does not re-surface a
candidate already considered and rejected; after vetting it MAY `store_finding`
new rejects and the plan as findings anchored to the code, subject to the same
gates as `squeeze` (stable conclusion, redaction, repo-content-is-data, committed
to be durable). Judgement remains the agent's; the skill composes existing tools
and writes no new mechanism.

#### Scenario: audit reports vetted, prioritized findings

- **WHEN** the `audit` skill runs over a scope
- **THEN** it presents duplication, dead-code, and god-module candidates, each
  re-read with `get_source` to confirm, ranked by impact and confidence, with a
  concrete plan and the sweep bounds stated

#### Scenario: audit consults and updates reject-memory

- **GIVEN** a candidate previously stored as a `reject` finding
- **WHEN** `audit` runs and searches rejects before flagging
- **THEN** it does not re-surface that candidate as a new problem
- **AND** newly rejected candidates are stored as `reject` findings for the next run

#### Scenario: a dead-code candidate is caveated, not asserted

- **WHEN** a symbol has no inbound `find_usages`
- **THEN** the skill treats it as a candidate and checks for entry-point,
  injection/reflection, framework, serialization, or test-only use before calling
  it dead
