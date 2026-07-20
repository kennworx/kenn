## Why

> Family B of the `skillset-roadmap` — the graph-understanding skills. `blast`
> is the most obviously useful pre-edit, because it composes `recall` with the
> call/type/usage graph the language indexers built.

`recall` answers "what steering governs these files?" by anchor match. But before
changing a symbol, the harder question is "**what will this touch?**" — the
transitive set of callers, usages, and implementers that a change ripples into.
kenn already exposes every hop of that graph (`list_callers`, `list_usages`,
`list_implementers`, `list_overrides`), but no skill *walks it into an answer*;
the `kenn` guide only lists the tools.

`blast` is the pre-edit blast-radius skill: from a symbol (or file) the agent is
about to change, it walks the graph transitively to compute the **change
surface**, then fuses `find_directives` on the touched files for the **rules that
govern them**. Change surface ∪ governing memory, in one shot — the graph
dimension `recall` lacks. Purely kenn-native; read-only.

## What Changes

- Add a **`blast` skill** (`claude-plugins/kenn/skills/blast/SKILL.md`):
  resolve the target to a symbol id, walk `list_callers` / `list_usages` /
  `list_implementers` / `list_overrides` transitively (bounded, with explicit
  truncation reporting — never a silent cap), collect the touched files/symbols,
  and run `find_directives` over those files. Present the change surface grouped
  by relation and the governing directives (heeding `polarity:dont`, flagging
  `stale`/`drifted`).
- Wire `blast` into the **kenn agent guide** as the graph-aware pre-edit skill,
  beside the lifecycle trio.

## Capabilities

### Modified Capabilities

- `agent-guide`: adds the `blast` skill requirement; the router names it as the
  pre-edit change-scope skill.

## Impact

- **Skills only** — markdown under `claude-plugins/kenn/skills/`; auto-discovered,
  no code or registration change.
- **No new tools** — `blast` composes existing graph + directive tools
  (`find_symbol`/`find_usages`, `list_callers`, `list_usages`,
  `list_implementers`, `list_overrides`, `find_directives`). Read-only: it
  orients, it does not write findings.
