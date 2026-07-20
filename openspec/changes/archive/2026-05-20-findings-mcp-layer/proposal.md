## Why

`lance-search-backend` and `findings-backend` are libraries — a committed hybrid-search store and a durable findings store with provenance. Agents cannot use them until they are exposed. The existing MCP server surfaces code-graph queries; this change adds the **knowledge layer**: MCP tools for hybrid search and for reading and writing findings, plus the **subagent-as-extractor** pattern — a main agent slices a task, fans out subagents that investigate and record findings, and synthesizes the results. The server stays dumb primitives: no model runs inside it; agents do all the reasoning. The findings store becomes the shared memory that carries knowledge across tasks, sessions, and orchestration stages.

## What Changes

- Expose MCP tools over the two backends:
  - **Read:** `semantic_search` (hybrid BM25 + vector over code *and* findings), `get_node`, `get_callers` / `get_callees`, `get_source`, `get_finding`, `search_findings`.
  - **Write:** `store_finding` (returns the new id plus near-duplicates), `merge_findings`.
  - **Provenance:** `find_predecessors` / `find_successors` to walk the derivation DAG.
- The server is **dumb primitives** — it holds no model and performs no task analysis. There is no `slice_for_task`-style tool; slicing and dispatch are the agent's job.
- Document the **subagent-as-extractor dispatch pattern**: a main agent decides how to slice a task, fans out general-purpose subagents in one message, each subagent investigates its slice through MCP and records findings, and returns finding ids. Subagents are autonomous within the MCP surface; coordination is via the findings store, not ad-hoc file passing.
- Ship a **system-prompt fragment** (a skill / instruction block) teaching an agent to search findings before re-investigating and to store a finding at a stable conclusion — the convention layer that makes findings accumulate as a byproduct of normal work.
- **Orchestrator-agnostic integration:** the tools work under any research → plan → implement → validate driver. The findings store is the cross-stage shared memory; a plan or a stage outcome can itself be stored as a finding, so later stages read earlier stages' reasoning.

## Capabilities

### New Capabilities
- `findings-mcp`: the agent-facing MCP surface for the knowledge layer — the hybrid-search, findings-read, findings-write, and derivation-DAG tools; the dumb-primitive guarantee (no model in the server); the subagent-as-extractor dispatch pattern; and the system-prompt fragment that drives finding accumulation.

### Modified Capabilities

None. The new tools are registered on the existing MCP server but form a self-contained capability; no existing `mcp-server` requirement (lifecycle, fail-fast-while-not-Ready, async dispatch) changes. Those guarantees apply to the new tools unchanged.

## Impact

- **Depends on:** `lance-search-backend` (hybrid search store) and `findings-backend` (findings store, derivation DAG, flush lifecycle).
- **Code:** `crates/kenn-mcp` — new tool handlers wired into the existing server; tool schemas; the system-prompt fragment shipped as a skill / instruction asset.
- **Agent workflow:** agents gain a knowledge layer that compounds — each task makes the next cheaper. No change to the existing code-graph tools.
- **Out of scope (follow-ups):** review-before-flush UX; orchestrator-specific adapters beyond the generic fragment; any model running server-side.
