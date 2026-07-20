## Why

> **Status: DEFERRED / future.** Recorded as a validated idea, not scheduled.
> Gate on real agent-usage evidence before building (see Design).

After `rrf-identifier-fusion`, the default search is one broad-ish query → RRF
ranking. For *narrowing* a broad result set, the primary axes for code are
already strong: graph traversal (callers / implementers / usages) and scalar
facets (language / project / kind / path). A **conjunctive lexical filter** —
"results must touch each of these concepts" — is a useful but minor additional
narrowing axis.

The shape is conjunctive normal form (an "AND of ORs"): an array of groups,
OR *within* a group (synonyms), AND *between* groups (required concepts):

```
 [[cancel, abort], [order, purchase]]  →  (cancel OR abort) AND (order OR purchase)
```

The OR-within-group is the key: the spike (`rrf-identifier-fusion` design D7)
showed plain AND collapses to zero on any extra/synonym word. Carrying synonyms
inside each required concept makes the AND robust.

## What Changes

- Add an **optional** structured query mode to the symbol-search tool: a
  `groups: [[..], ..]` parameter that builds a CNF lexical filter (OR within,
  AND between) over the lexical (FTS5) arms. Plain-string query + RRF ranking
  stays the default.
- The filter is **precision-over-recall**: it narrows, it does not re-rank. It
  must include a **relaxation fallback** — if the AND-of-groups returns too few
  results, drop a group (or fall back to OR) rather than return nothing.
- The semantic (vector) arm does not take boolean structure; when a structured
  query is used, the vector arm operates on a natural-language **flattening** of
  the groups (or is omitted from the filtered pass).

## Capabilities

### Modified Capabilities

- `mcp-symbol-search`: gains an optional CNF lexical-filter query mode (narrow),
  distinct from the default ranking query.

## Impact

- **Behavior:** agents can narrow a broad result set by required concepts; the
  default ranking path is unchanged.
- **Open / unproven:** this is the one design with no self-supervised
  validation — its value depends on agents formulating good groupings, which
  needs real query logs + outcomes, not a corpus spike. Build only after that
  evidence exists.
