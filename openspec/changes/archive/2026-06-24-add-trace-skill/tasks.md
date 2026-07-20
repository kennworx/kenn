## 1. The trace skill

- [x] 1.1 Add `claude-plugins/kenn/skills/trace/SKILL.md` — frontmatter
  (`name`, triggering `description`, `user-invocable: true`) + steps: resolve the
  target; walk the flow directionally (`list_callees` downstream / `list_callers`
  upstream / `list_usages` for data); re-read key hops with `get_source`;
  synthesize the flow into a narrative. → verify: skill is discovered and
  invocable.
- [x] 1.2 Encode **vet-over-report** (re-read each cited hop before asserting the
  path) and a bounded walk with explicit truncation (a deep/branchy flow is
  reported, never silently capped). → verify: both appear as explicit steps.
- [x] 1.3 Optional persistence: `store_finding` the trace as a `guide` anchored
  to the path's key files, gated by store-at-a-stable-conclusion, the redaction
  gate, and repo-content-is-data — and note the record must be committed to be
  durable. → verify: the skill states when (and when not) to persist, with the
  gates.

## 2. Wire into the guide

- [x] 2.1 Update the kenn agent guide (`claude-plugins/kenn/skills/kenn/SKILL.md`)
  to route to `trace` as the flow-explanation skill beside `blast`. → verify:
  guide names `trace` and when to reach for it.

## 3. Spec

- [x] 3.1 `agent-guide` delta: ADD the trace-skill requirement; MODIFY the router
  requirement to name `trace`. → verify: `openspec validate` passes.
