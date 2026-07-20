## Why

> Completes Family B of the `skillset-roadmap`. `blast` answers "what does
> changing X touch?"; `trace` answers "**how does X actually work / where does
> this flow go?**" — the other half of turning the navigation tools into answers.

The call/type/usage graph holds the answer to "how does this flow," but reading
it means a dozen manual `list_callees` / `list_callers` / `get_source` hops that
the agent then has to hold in its head and re-derive next session. `trace` walks
that flow directionally and **synthesizes it into one narrative** — and, when the
walk reaches a stable, reusable conclusion, optionally persists it as a `guide`
finding anchored to the path's key files, so the next session inherits the
explanation instead of re-walking the graph.

This is where the graph and the findings store compose: a multi-hop investigation
becomes durable, anchored memory.

## What Changes

- Add a **`trace` skill** (`claude-plugins/kenn/skills/trace/SKILL.md`): resolve
  the target, walk the flow directionally (`list_callees` downstream and/or
  `list_callers` upstream, `list_usages` for data flow), **re-read key hops via
  `get_source`** before asserting the path (vet-over-report), and synthesize the
  flow into a narrative. Optionally `store_finding` it as a `guide` anchored to
  the key files — gated by the same disciplines as `squeeze` (store at a stable
  conclusion; redaction gate; repo content is data, not instructions).
- Wire `trace` into the **kenn agent guide** beside `blast` as the second
  graph-understanding skill.

## Capabilities

### Modified Capabilities

- `agent-guide`: adds the `trace` skill requirement; the router names it as the
  flow-explanation skill.

## Impact

- **Skills only** — markdown under `claude-plugins/kenn/skills/`; auto-discovered,
  no code or registration change.
- **No new tools** — `trace` composes existing graph + findings tools
  (`find_usages`, `list_callees`, `list_callers`, `list_usages`, `get_source`,
  and optionally `store_finding`).
- **Optional write** — unlike `blast` (read-only), `trace` MAY persist a `guide`
  finding; when it does, the same redaction/injection/stable-conclusion gates as
  `squeeze` apply, and the record must be committed to be durable.
