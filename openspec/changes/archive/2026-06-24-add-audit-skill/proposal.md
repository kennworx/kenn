## Why

> The capstone of Family C and the `skillset-roadmap`. Its gate — "`reconcile`
> proves the lifecycle loop" — is met: `reconcile` was run end-to-end on a
> 71k-symbol repo (broken/drifted/stale all validated), and the dogfood hardened
> the machinery underneath it (three fixes). `audit` is now worth its cost.

`audit` borrows the `improve` pipeline (recon → audit → vet → prioritize → plan),
but its mechanical legs **query the code graph** instead of grepping:
- **duplication** via `find_similar` (the `dup` sweep),
- **dead code** via symbols with no inbound `find_usages`,
- **god-modules** via `list_callers` fan-in and module size (the index already
  counts `god_nodes`/`communities` in the overview),
- **vet** via `get_source` — re-read every candidate before it makes the report.

The payoff unique to kenn: the **"considered and rejected" memory becomes
queryable findings**. A candidate the agent judges *not* a real problem is stored
as a `reject`-tagged finding anchored to the code, so the next audit searches that
memory first and does not re-flag it — replacing `improve`'s throwaway
markdown-section reject list with durable, searchable state.

## What Changes

- Add an **`audit` skill** (`claude-plugins/kenn/skills/audit/SKILL.md`): the
  staged pipeline over a scope, each mechanical leg graph-backed, every finding
  vetted via `get_source`, ranked by impact × confidence, ending in a concrete
  plan. Sweeps are bounded with explicit truncation.
- **Reject-memory**: before flagging, `search_findings` for prior `reject`s;
  after vetting, store new rejects and the consolidation/removal plan as findings
  (under the `squeeze` gates). Dead-code candidates carry the standard caveats
  (entry points, DI/reflection, framework hooks, serialization, tests).
- Wire `audit` into the **kenn agent guide** as the advisor-family deep audit,
  noting it composes `dup` and the graph tools.

## Capabilities

### Modified Capabilities

- `agent-guide`: adds the `audit` skill requirement; the router names it as the
  deep, graph-backed codebase audit with reject-memory.

## Impact

- **Skills only** — markdown under `claude-plugins/kenn/skills/`; auto-discovered,
  no code or registration change.
- **No new tools** — composes `get_workspace_overview`, `find_similar`,
  `find_usages` / `list_usages`, `list_callers`, `list_in_scope` /
  `list_module_files`, `get_source`, `search_findings`, `store_finding`.
- **Writes findings** (plan + reject-memory) — subject to the `squeeze` gates
  (stable conclusion, redaction, repo-content-is-data, committed to be durable).
