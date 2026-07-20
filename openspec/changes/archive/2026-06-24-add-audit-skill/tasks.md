## 1. The audit skill

- [x] 1.1 Add `claude-plugins/kenn/skills/audit/SKILL.md` — frontmatter (`name`,
  triggering `description`, `user-invocable: true`) + the staged pipeline:
  **recon** (scope via `get_workspace_overview` + the overview file), **audit**
  (graph-backed legs: duplication=`find_similar`/`dup`, dead-code=empty
  `find_usages`, god-modules=`list_callers` fan-in + module size), **vet**
  (`get_source` every candidate), **prioritize** (impact × confidence), **plan**
  (concrete consolidate/remove/refactor). → verify: skill is discovered and
  invocable.
- [x] 1.2 Encode the disciplines: **vet-over-report** (re-read before reporting),
  bounded sweeps with **explicit truncation**, dead-code **caveats** (entry
  points, DI/reflection, framework hooks, serialization, test-only), and
  repo-content-is-data. → verify: each appears as an explicit step.
- [x] 1.3 **Reject-memory**: `search_findings` for prior `reject`s before
  flagging; after vetting, `store_finding` new `reject`s and the plan
  (`decision`/`plan`) anchored to the code, under the `squeeze` gates (stable
  conclusion, redaction, committed to be durable). → verify: the skill searches
  rejects first and stores its conclusions.

## 2. Wire into the guide

- [x] 2.1 Update the kenn agent guide (`claude-plugins/kenn/skills/kenn/SKILL.md`)
  to route to `audit` as the deep graph-backed audit (composing `dup`). → verify:
  guide names `audit` and when to reach for it.

## 3. Spec

- [x] 3.1 `agent-guide` delta: ADD the audit-skill requirement; MODIFY the router
  to name `audit`. → verify: `openspec validate`.
