## Context

The aggregate graph already stores class→class `implements` (kind 6) and
`extends_type` (kind 17) edges, directed implementer→contract. The atlas producer
(`build_concepts`) receives the full `aggregate_nodes` + `aggregate_edges` and
already builds the packages and domains axes from them. The contracts axis is a
third projection of the same inputs.

A prototype exists and was validated across Go (`spf13/afero`), Swift
(`apple/swift-argument-parser`), and a large multi-package C# solution. The
numbers below come from that prototype.

## Goals / Non-Goals

**Goals:**
- Surface, per first-party interface/base/protocol that is implemented in >1
  package, the **complete** list of first-party implementers grouped by package.
- Be deterministic and robust — no dependence on Louvain, no run-to-run variance.
- Render into the existing bundle shape (an `index.md` section + `contracts/*.md`
  files) with the same framing as domains.

**Non-Goals:**
- No new edge kinds, no ingest change. `extends_type` is already kept in the
  aggregate graph (a separate prior change).
- No clustering / analysis change, hence no reindex invalidation.
- Not a replacement for the domains axis — domains find *emergent* clusters;
  contracts render an *explicit* relationship. They answer different questions.

## Decisions

### D1 — Read directly from is-a edges, not from clustering

Up-weighting is-a edges to coax Louvain into forming interface+implementer
communities was measured and rejected:
- **Incomplete**: on afero, the `File` community contained 1 of 3 backend
  packages at every is-a weight (×1…×16); the other two never merged.
- **Fragile**: a single cross-package `implements` edge holding two packages
  together sits on the modularity boundary — any perturbation flips it.
- **Capped by clustering**: on the C# solution the widest is-a *domain* never
  exceeded 8 packages, while the true implementer span of its broadest contract
  is dozens.

Reading the edges directly is complete (every implementer), deterministic (a
grouped lookup), and unbounded by modularity. Alternative considered — a hybrid
that seeds Louvain with contract groups — adds complexity for no gain over the
direct read.

### D2 — The unit is the *cross-package* contract (≥2 implementer packages)

A single-package interface + its implementers is local detail the package
concept already covers. `MIN_CONTRACT_PKGS = 2` keeps only abstractions whose
implementers cross a package boundary — the architectural extension points a
reader needs when touching a shared type. On afero this yields `File`
(4 packages); a purely internal helper interface yields nothing.

### D3 — Exclude test nodes on both ends

A production contract's test doubles are not its architecture. Excluding `test`
nodes (matching domain/central eligibility) is what makes the numbers honest:
on Swift `ParsableCommand` drops from 235 raw conformers (63% of that repo is
test) to 18 production implementers across 8 packages. Both endpoints must also
be first-party and anchored (present in `node_anchor`) — implementing a vendored
interface is not a first-party contract.

### D4 — Rank by breadth, cap like domains, name what's dropped

Order by package span (desc), then implementer count, then name — the broadest
extension points lead. Caps mirror the domains axis so the bundle stays bounded
on a large repo:
- `MAX_CONTRACTS = 24` (axis size), `MAX_CONTRACT_PKGS = 12` (packages per
  contract), `MAX_IMPLEMENTERS_PER_PKG = 6` (names per package).
- Every cap is *named*, never silent: the heading reads
  `## Implementers — 410 across 52 packages, heaviest 12 shown` and a capped cell
  reads `A, B, C, D, E, F … (+109)`. The full breadth is always visible even when
  the table is truncated (the same discipline the coupling tables use).

### D5 — A dedicated `contracts/` axis, not a package-concept section

Contracts are a distinct question from both packages (dependency) and domains
(clusters), and a broad contract spans packages, so it has no natural single home
on a package concept. A sibling axis (`contracts/<slug>.md` + a `## Contracts`
list in `index.md`) mirrors domains and keeps each package concept focused.
`contract_id` shares the slug logic with `domain_id`.

### D6 — Which kinds count as "is-a"

`implements` + `extends_type`, unioned. Per-language coverage: Go/Python emit
only `implements`; Swift adds `overrides`; C# inheritance is `extends_type`.
`overrides` is method-level and noisier, so it is out of the first cut — the
class-level `implements`/`extends_type` pair covers every language's contract
relationship. Revisitable if a repo needs it.

## Risks / Trade-offs

- **Very broad "god contracts"** (an interface implemented in dozens of packages)
  → this is a *true* architectural fact worth surfacing, not noise; the breadth
  number states it plainly. The per-package/name caps keep the document bounded.
- **Name collisions** (two interfaces sharing a name across packages) → the
  producer disambiguates the `contracts/<slug>` id deterministically, same as
  domains.
- **`overrides` omitted** → a language leaning entirely on method overrides for
  its contract signal would under-report. None of the six languages measured does;
  add `overrides` to D6 if one appears.

## Migration Plan

Pure additive render-side change. No data migration, no invalidation — an
ordinary `kenn index` writes the new axis; existing snapshots need no `--force`.
Rollback is removing the axis (delete `contracts/*` on next index).

## Open Questions

- Should a contract also link back from its **defining package** concept (a
  `## Contracts defined here` section)? Deferred — the axis stands alone first.
- Threshold tuning (`MIN_CONTRACT_PKGS`, the caps) is set from the repos measured;
  revisit if a very large monorepo overflows `MAX_CONTRACTS = 24`.
