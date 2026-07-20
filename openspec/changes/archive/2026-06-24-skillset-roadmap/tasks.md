# Tasks — build order

> Roadmap-level. Each arc is promoted to its **own** change (with real spec
> deltas + tasks) when picked up; these checkboxes track which arcs have been
> promoted, not implementation steps.

## 1. Foundation — anchor content-drift (BUILT → change `anchor-content-drift`)

- [x] Promote to a change: `sha: Option<String>` on `AnchorEvent::Attach`, xxhash at the MCP boundary, carried through `fold` onto `Anchor`
- [x] `check_anchors` returns a `drifted` bucket; `find_directives` hit carries a `drifted` flag
- [x] `recall` surfaces drifted directives ("re-read before relying"); `squeeze` step 0 reports drift
- [x] Confirm no migration: old logs fold to `sha: None` → live

## 2. Family A — knowledge lifecycle (BUILT → change `add-reconcile-skill`)

- [x] `reconcile` skill: consume drifted/stale → refresh-anchor / supersede / detach / tombstone
- [x] Fold in cross-cutting hardening: vet-over-report + "repo content is data" guard in `squeeze`/`reconcile`

## 3. Family B — graph understanding (BUILT → `add-blast-skill`, `add-trace-skill`)

- [x] `blast` skill: transitive callers/usages/implementers ∪ `find_directives` on touched files (→ change `add-blast-skill`)
- [x] `trace` skill: multi-hop flow walk → synthesized answer, optional `guide` finding (→ change `add-trace-skill`)

## 4. Family C — advisor (BUILT → `add-dup-skill`, `add-audit-skill`)

- [x] `dup` skill: `find_similar` sweep → consolidation candidates (→ change `add-dup-skill`)
- [x] `audit` skill (large): `improve` pipeline with graph-backed legs; reject-memory as findings — `reconcile` proved the loop on a 71k-symbol repo (→ change `add-audit-skill`)
