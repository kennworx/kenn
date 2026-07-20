## 1. The dup skill

- [x] 1.1 Add `claude-plugins/kenn/skills/dup/SKILL.md` — frontmatter (`name`,
  triggering `description`, `user-invocable: true`) + steps: pick the scope;
  enumerate seed symbols (`list_in_scope` / `search_symbols`); sweep
  `find_similar` on each; dedup pairs into clusters; present consolidation
  candidates. → verify: skill is discovered and invocable.
- [x] 1.2 Encode **vet-over-report**: re-read every candidate with `get_source`
  before calling it a duplicate (embedding nearness is a candidate, not a
  verdict), and a bounded sweep with explicit truncation (seed cap / large scope
  reported, never silently capped). → verify: both appear as explicit steps.
- [x] 1.3 Optional persistence: `store_finding` a consolidation `decision`/`plan`
  under the `squeeze` gates (stable conclusion, redaction, repo-content-is-data,
  must-commit). → verify: the skill states when to persist, with the gates.

## 2. Wire into the guide

- [x] 2.1 Update the kenn agent guide (`claude-plugins/kenn/skills/kenn/SKILL.md`)
  to route to `dup` as the duplication-sweep advisor skill. → verify: guide names
  `dup` and when to reach for it.

## 3. Spec

- [x] 3.1 `agent-guide` delta: ADD the dup-skill requirement; MODIFY the router
  requirement to name `dup`. → verify: `openspec validate` passes.
