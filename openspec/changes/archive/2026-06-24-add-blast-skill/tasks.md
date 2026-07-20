## 1. The blast skill

- [x] 1.1 Add `claude-plugins/kenn/skills/blast/SKILL.md` — frontmatter
  (`name`, triggering `description`, `user-invocable: true`) + steps: resolve the
  target to a symbol id; walk `list_callers` / `list_usages` /
  `list_implementers` / `list_overrides` transitively to the change surface;
  collect touched files; run `find_directives` over them; present surface ∪
  governing directives. → verify: skill is discovered and invocable.
- [x] 1.2 Bound the walk and **report truncation explicitly** (hop limit / large
  fan-in is stated, never a silent cap), and flag `stale`/`drifted` directives
  and `polarity:dont` rules in the governing set. → verify: the skill states its
  bound and surfaces the flags.

## 2. Wire into the guide

- [x] 2.1 Update the kenn agent guide (`claude-plugins/kenn/skills/kenn/SKILL.md`)
  to route to `blast` as the pre-edit change-scope skill (the graph counterpart
  to `recall`). → verify: guide names `blast` and when to reach for it.

## 3. Spec

- [x] 3.1 `agent-guide` delta: ADD the blast-skill requirement; MODIFY the router
  requirement to name `blast`. → verify: `openspec validate` passes.
