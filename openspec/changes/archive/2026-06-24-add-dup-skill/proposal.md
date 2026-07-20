## Why

> Opens Family C (advisor) of the `skillset-roadmap` — the tractable taste-test
> before the large `audit` arc. It is `find_similar`'s headline use case.

`find_similar` returns the symbols whose committed embedding is nearest a given
symbol's — it is *built* for "parallel implementations with no shared call edge,"
the look-alike logic that grep and the call graph both miss (different names, no
caller in common). But no skill turns that into a duplication review; the `kenn`
guide only lists the tool.

`dup` is a focused duplication sweep: over a scope (a module, a directory, or a
seed symbol), run `find_similar`, keep the near-duplicate clusters, **re-read each
candidate via `get_source`** to confirm the logic is genuinely duplicative (not
just similarly named), and present consolidation candidates. It gets for free,
from the index, what a manual audit would grep for.

## What Changes

- Add a **`dup` skill** (`claude-plugins/kenn/skills/dup/SKILL.md`): pick the
  scope, enumerate seed symbols (`list_in_scope` / `search_symbols`), sweep
  `find_similar` on each (bounded, truncation reported), dedup the pairs into
  clusters, **vet each cluster with `get_source`** (vet-over-report — semantic
  nearness is a candidate, not a verdict), and present consolidation candidates
  ranked by confidence. Optionally record a consolidation `decision`/`plan`
  finding (under the `squeeze` gates) so the call survives the session.
- Wire `dup` into the **kenn agent guide** as the advisor-family duplication
  skill.

## Capabilities

### Modified Capabilities

- `agent-guide`: adds the `dup` skill requirement; the router names it as the
  duplication-sweep advisor skill.

## Impact

- **Skills only** — markdown under `claude-plugins/kenn/skills/`; auto-discovered,
  no code or registration change.
- **No new tools** — `dup` composes `find_similar`, `list_in_scope` /
  `search_symbols`, `get_source`, and optionally `store_finding`.
- **Vet-required** — `find_similar` ranks by embedding proximity, which surfaces
  candidates; the skill MUST re-read source before calling anything a duplicate,
  to avoid false positives from shared vocabulary.
