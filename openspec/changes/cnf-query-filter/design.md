# Design — CNF query filter (deferred)

## Why deferred

Every other change this cycle was settled by a self-supervised gold set. The
value of CNF filtering hinges on a question a corpus spike cannot answer: **do
agents construct good (synonym-OR, concept-AND) groupings?** That needs real
agent query logs + outcomes (a usage eval). Building on speculation is the one
move that would not be evidence-backed, so this is recorded and parked.

## What the spike already established

- **It's a filter, not a ranking win.** For ranking, AND = OR (BM25-over-OR
  already surfaces full-coverage matches — `rrf-identifier-fusion` design D7). So
  CNF will not improve recall/MRR; it narrows a broad set to the items touching
  every required concept.
- **OR-within-group cures AND-brittleness.** Plain AND collapsed to 0 on one
  extra word; synonyms inside each group keep every required concept matchable.
- **For code, lexical narrowing is the *least* important axis.** The agent's
  strong narrowing levers are graph traversal and scalar facets; CNF sits below
  them. This bounds the upside and the priority.

## Design constraints (if/when built)

**C1 — Optional, default unchanged.** A `groups` parameter; plain-string query +
RRF ranking remains the default and the common path.

**C2 — Lexical-only, with NL flatten for the vector arm.** CNF is an FTS5
construct. The vector arm embeds whole text and has no AND/OR; under a structured
query it operates on a natural-language flattening of the groups, or is omitted
from the filtered pass. This impedance mismatch is the main reason CNF is a
*filter* layered on the lexical arms, not a fusion arm.

**C3 — Mandatory relaxation fallback.** If the AND-of-groups returns too few
results, drop the least-selective group (or relax to OR) before returning empty.
Without this, AND-brittleness reappears at the group level and the agent just
re-queries.

**C4 — Build through the `fts5_match` normalizer** (`rrf-identifier-fusion`
design D8): each group is a normalized OR of quoted tokens; groups are AND-joined
with parentheses. Injection-safe by construction.

## Open questions to resolve with usage data

- Do agents over-constrain (too many AND groups → empty) or under-use it?
- Is the relaxation policy (drop which group?) good enough, or does it need
  selectivity estimates?
- Does CNF earn its tool-surface cost given graph + scalar narrowing already
  exist?
