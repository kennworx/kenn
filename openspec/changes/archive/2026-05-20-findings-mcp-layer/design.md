## Context

Two backends are in place: `lance-search-backend` (committed hybrid-search store over code) and `findings-backend` (durable findings with a provenance DAG). They are libraries. Agents reach the codebase through the MCP server, which today exposes code-graph queries.

This change is the agent-facing surface for the knowledge layer. The hard questions are not storage — those are settled — but interaction design: what the tools are, where the intelligence lives, and how a main agent turns a task into accumulated, provenance-tracked knowledge. The earlier exploration converged on a clear stance; this records it.

## Goals / Non-Goals

**Goals:**
- A minimal MCP tool surface over the two backends — read, write, and provenance.
- Keep the server a set of dumb primitives; all reasoning is in the agents.
- A repeatable subagent-as-extractor pattern for fanning out investigation.
- Make findings accumulate as a byproduct of ordinary tasks, under any orchestrator.

**Non-Goals:**
- Running any model inside the MCP server.
- A built-in task planner or work-slicer tool.
- Orchestrator-specific integrations beyond a generic system-prompt fragment.
- Review-before-flush UX.

## Decisions

### D1 — The server is dumb primitives; intelligence is in the agents

The MCP server exposes capability, never judgement. There is no `slice_for_task`, no `decide_strategy` — those need a model, and the server holds none. The server reads the graph, reads and writes findings, and walks the DAG. A main agent composes those primitives: it interprets the task, decides how to slice it, dispatches subagents, and synthesizes. This keeps the server simple, testable, and orchestrator-agnostic. Alternative considered — a server-side planner — was rejected: it would put a model in the server and couple it to one orchestration style.

### D2 — The tool surface

```
read       semantic_search(query, scope, k)   hybrid BM25 + vector, code and/or findings
           get_node(id) · get_callers(id) · get_callees(id) · get_source(id)
           get_finding(id) · search_findings(query)
write      store_finding(text, parent_ids, tags) -> { id, similar[] }
           merge_findings(ids, text, tags) -> id
DAG        find_predecessors(id) · find_successors(id)
```

`store_finding` returns near-duplicates so the agent can choose to supersede, link, or accept (the `findings-backend` contract). `parent_ids` span the unified ID space, so a finding can cite code nodes and prior findings alike. The surface is intentionally small — every tool is a primitive, none is a workflow.

### D3 — Subagent-as-extractor dispatch

For a task that decomposes into independent investigations, a main agent:

1. uses `semantic_search` + graph reads to orient and pick anchors,
2. decides the slicing — how many subagents, what each investigates,
3. fans out general-purpose subagents in one message,
4. each subagent works autonomously through the MCP surface and records findings via `store_finding`, returning their ids,
5. the main agent collects the ids and synthesizes, optionally via `merge_findings`.

Coordination is through the findings store and returned ids — not ad-hoc file passing. The dispatch decision is the agent's; the server is uninvolved. Dispatch is worth it only when a task has genuinely independent sub-investigations — a single-anchor lookup needs no fan-out.

### D4 — The findings store is cross-stage shared memory

Under a research → plan → implement → validate orchestrator, each stage reads predecessors' findings and writes its own, linked by `parent_ids`. A plan is itself a finding (`kind:plan` tag) whose parents are the research findings; an implementation note's parents include the plan. The derivation DAG thus threads the whole task, and "why was this done?" is answerable across stages. The server enables this purely by hosting the findings tools — it has no concept of a stage.

### D5 — The system-prompt fragment is the convention layer

Findings accumulate only if agents know to record them. A short, shippable system-prompt fragment instructs an agent to (a) `search_findings` before re-investigating and (b) `store_finding` at a stable conclusion — not after every thought. This fragment, not server logic, is what makes the knowledge layer self-populating. It is the highest-leverage and most tuning-sensitive deliverable; phrasing is expected to be iterated against real runs.

### D6 — Free-string tags, no enforced vocabulary

`tags` on `store_finding` are free strings. Per-task or per-orchestrator vocabularies (`evidence`, `gotcha`, `plan`, `decision`) emerge by convention, suggested in the fragment, never enforced by the protocol. This matches the `findings-backend` decision to avoid a `kind` enum.

## Risks / Trade-offs

- **Subagent dispatch is harness-specific** — fanning out subagents in one message is a Claude Code `Agent`-tool capability; other harnesses differ. → Mitigation: the MCP tools are universal; the dispatch *pattern* is documented as Claude-Code-targeted, and degrades to single-agent use elsewhere.
- **Prompt-fragment tuning** — too aggressive and agents store noise; too passive and they store nothing. → Mitigation: ship a first version, treat it as tunable, measure against real tasks. The `store_finding` similarity check is a backstop against duplicate noise.
- **Over-storing low-value findings** → Mitigation: the fragment says store at *stable conclusions*, not intermediate thoughts; supersede/tombstone clean up later.
- **No server-side guardrail on slicing** — a main agent could fan out badly. → Mitigation: accepted; the server is deliberately not a planner. Slicing quality is an agent-prompt concern.

## Migration Plan

- Purely additive: new tools registered on the existing MCP server; existing code-graph tools and lifecycle behavior unchanged.
- The server's existing guarantees (fail-fast while not Ready, async dispatch, progress notifications) apply to the new tools without modification.
- Rollback: revert the change; the new tools disappear, the backends and the existing tools are unaffected.

## Open Questions

- **Tag vocabulary defaults.** What starter vocabulary the fragment suggests for the three task classes (fix / feature / comprehension). Settle from early runs.
- **When agents flush.** The fragment must say when to invoke the findings flush — at task end, at stage boundaries, or on explicit request. Lean: at a stable conclusion / stage boundary.
- **Merge timing.** Does each subagent `merge_findings` before returning, or does the main agent collect raw finding ids and merge centrally? Lean: subagents return raw ids, the main agent merges — full visibility for synthesis.
- **Fragment packaging.** Shipped as a skill, an instruction file, or both — decide with the `kenn-mcp` install surface.
