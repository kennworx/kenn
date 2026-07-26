## Context

The overview's counts are specified to come from the build-time `stats` table
with no read-path aggregation:

> The overview SHALL perform no **database** aggregation on the read path — no
> `SUM`, `count(*)`, `GROUP BY`, or `count_table` query — it only reshapes the
> rows `stats()` returns.

So an earned domain count must be a `stats` row written at index time. Today it
cannot be, and the obstruction is crate topology rather than logic:

- `kenn-analyze::graph_stat_rows(nodes, recs)` writes the whole-graph counters,
  but takes no edges. The earned-span rule needs edges — a package earns its place
  in a domain only by having a first-party edge to another qualifying package.
  This turns out to be a local signature change, not a data problem: the analysis
  hook is `Fn(nodes, edges, writer)`, so the edges are already in scope at the
  call site and are already materialized to build the graph. Passing them to
  `graph_stat_rows` adds no traversal, which matters because the graph-analysis
  spec requires these counters be derived with **no extra graph traversal**.
- `kenn-indexer/src/atlas/` owns the rule, but the atlas is an
  `Option<AtlasContext>` in the pipeline; a stat row written there would exist
  only on runs that built the atlas.
- Neither crate depends on the other. Both are siblings over
  `kenn-model`/`kenn-config`/`kenn-store`, and the analysis reaches the indexer
  through an injected `post_aggregate_hook`. That looks deliberate — the indexer
  can run without linking the analysis crate.

All the `*Record` types the rule operates on (`AggregateEdgeRecord`,
`AnalysisFlatCommunityRecord`, `AnalysisNodeMembershipRecord`) already live in
`kenn-model`, so the DATA is universally reachable. Only the LOGIC is stranded.

## Goals / Non-Goals

**Goals:**

- The overview cannot be read as claiming a domain count it does not mean.
- The earned count comes from `stats`, preserving the no-aggregation-on-read rule.
- One implementation of the earned-span rule, reachable by every consumer that
  needs it: the atlas, the domains query, and the stats writer.

**Non-Goals:**

- Retuning the floors. This change decides where the rule LIVES and who counts
  with it; the rule's semantics are settled.
- Removing `cross_anchor_communities`. It is a legitimate clustering signal and
  the graph-analysis spec defines it; dropping a field is a harsher break than
  adding one.

## Decisions

### D1 — Report both counters, named; do not redefine the existing one

The overview reports the raw counter under its current name and meaning, and adds
the earned domain count beside it. A reader seeing `38 raw / 9 domains` learns
something true about how much the floors filter; a reader seeing only `38` was
misled, and a reader seeing `9` under the name `cross_anchor_communities` would be
misled differently — the stats table would then disagree with the overview.

### D2 — RESOLVED: the rule does not move. `kenn-indexer` writes the row.

The open question below asked whether the indexer's independence from
`kenn-analyze` is incidental. **It is deliberate, and normative** — so option A
is not merely unattractive, it is forbidden:

- `openspec/specs/atlas-bundle/spec.md`: the producer *"SHALL NOT recompute
  clustering nor depend on `kenn-analyze`; kenn-indexer and kenn-analyze stay
  parallel consumers of the persisted graph."*
- Directive `fnd_eb7b643d` states the same rule independently.
- The reverse edge is equally deliberate: `kenn-analyze/src/lib.rs` duplicates
  the eight-line `PostAggregateHook` type alias explicitly *"without the dep on
  `kenn-indexer`"*. Paying duplication is how you know a constraint is intended.

Git history cannot settle this either way — everything predates the squashed
initial commit — so the spec and that comment are the whole record.

The constraint is therefore **symmetric**: neither crate may depend on the other.
That kills A, and it would force B (rule into `kenn-model`) or a new shared
crate — *if* `kenn-analyze` had to be the one to compute the count. It doesn't:

**Option E (chosen): `kenn-indexer::aggregate::compute_and_persist` computes the
earned count and writes the stat row, after the analysis hook, reading the
communities back off the writer's own connection.**

This is the pattern the atlas already uses and the spec already blesses —
`aggregate.rs` calls `writer.scan_analysis_node_membership()` /
`scan_analysis_flat_communities()` today, annotated *"atlas ⊥ kenn-analyze — it
consumes the persisted tables, never recomputes clustering."* Extending it to a
counter adds no dependency, no crate, and no traversal:

| | A | B | C | **E** |
|---|---|---|---|---|
| new deps | ✗ forbidden | none | none | **none** |
| rule moves | yes | yes | no | **no** |
| counter always present | yes | yes | ✗ atlas-only | **yes** |
| policy in a types crate | no | ✗ yes | no | **no** |

Two things make E cheap that were not true when this was written:

1. `atlas::domains::select_domains` is already extracted and input-agnostic
   (done by `atlas-axes-on-the-cli`), and already shared with the domains query.
   Nothing needs relocating for a second caller in the same crate.
2. Node eligibility is now computable from the aggregate node rows ALONE —
   `example` became a persisted node fact in `persist-node-example-provenance`,
   so the caller no longer needs the atlas's `primary_def_file` → `files` joins
   to answer `is_domain_eligible`.

**Accepted trade-off:** the raw counter is written by `kenn-analyze` and the
earned one by `kenn-indexer`, so two producers write adjacent rows in the same
`scope='global', subset='graph'` group. Directive `fnd_08f51841` already
establishes multiple producers behind one `DbWriter::write_stats`, so this is the
existing shape rather than a new one — but a reader of `stats` cannot tell which
component wrote which row, and that is a genuine (small) loss of locality.

**Corollary:** the row is written only when the analysis pass actually produced
communities. Absent means "clustering did not run" — exactly the condition under
which `cross_anchor_communities` is also absent, so the two counters appear and
disappear together and can never be compared across different runs.

### D2 (original, superseded) — Where the rule lives: OPEN, decided before implementation

Three candidates, with the trade-off that actually distinguishes them:

| Option | New deps | Always present? | Cost |
|---|---|---|---|
| **A.** Rule moves to `kenn-analyze`; atlas imports it | `kenn-indexer → kenn-analyze` | yes (whenever analysis ran) | inverts the current injection design |
| **B.** Rule in `kenn-model` | none | yes | selection policy in a data-model crate |
| **C.** Atlas writes the stat row | none | **no** — only when the atlas ran | counter becomes conditional |

**C is disqualified for the stated goal**: a counter that appears only on runs
that built the atlas is a worse contract than the inconsistency being fixed.

Note this narrows to a pure question of where the RULE lives, not where the data
is: `kenn-analyze` already has the edges, the nodes, and the community records it
needs. Under either A or B the computation happens in the analysis pass; they
differ only in which crate the shared module sits in.

Between A and B: A puts the rule next to the clustering that produces the
communities and the `cross_anchor` predicate it strictly refines, which is where a
reader would look for it. B is free but puts a threshold-bearing policy decision in
the crate that is otherwise pure types.

~~Leaning A, conditional on confirming the indexer's independence from
`kenn-analyze` is incidental rather than a deliberate constraint — worth checking
the change history for `post_aggregate_hook` before committing.~~

**Checked. It is deliberate and normative, so A is out and this whole A-vs-B
framing is moot — see the resolution at the head of D2. `post_aggregate_hook`
has no history to read (squashed initial commit); the record is
`atlas-bundle/spec.md` plus the duplicated type alias in `kenn-analyze`.**

### D2a — CORRECTION found in verification: the two counters are NOT nested

The `## Why` framing ("raw communities systematically overstate ... the 9 are the
domains that clear the axis floors") implies `domains ⊆ cross_anchor_communities`.
**It is not a subset relation, and asserting it would be wrong.**

Measured across six real repos after implementing:

| repo | raw | earned |
|---|---|---|
| this workspace | 40 | 10 |
| Go, 24 packages | 20 | 2 |
| C#, 125 packages | 284 | 78 |
| TypeScript, single package | **0** | **8** |
| Python, single package | **0** | **20** |
| Python, single package | **0** | **13** |

A single-package repo has nothing spanning two anchors, so the raw cross-anchor
count is 0 — while the domain axis deliberately keeps WITHIN-anchor clusters for
a monolithic library (`single_dominant`), or a one-package repo would have no
domains at all. So the earned count legitimately EXCEEDS the raw one there.

Consequence for D1: the pair is not "a number and its refinement", it is two
questions over different candidate sets. Both are published under their own
names, neither is derived from the other, and no ordering invariant may be
asserted between them — in a test, a doc, or a reader's head. `GraphSummary`'s
docs state this at the point of use.

### D3 — The earned count is computed once, at index time, and read by both

Whichever crate hosts the rule, the count is produced during the analysis/aggregate
phase and written as a `scope='global', subset='graph'` stat row, matching the
existing whole-graph counters. The domains query does NOT read it — it computes
membership itself from the same rule, so a query and the counter cannot disagree
by construction.

## Risks / Trade-offs

- **Adding `kenn-indexer → kenn-analyze` reverses an intentional decoupling** →
  Check why the hook injection exists before choosing A. If the indexer is meant
  to be runnable without the analysis crate, B is the answer despite the layering
  smell.
- **Two counters invite confusion of a different kind** ("which do I want?") →
  Mitigate in the field names and the tool description, not in a comment: the raw
  one is a clustering diagnostic, the earned one is the axis. The scenario below
  pins that they differ on a real snapshot.
- **The earned count silently drifts from `list_domains`** → Both derive from the
  single rule (D3). A test asserts the stat equals what the query returns for the
  same snapshot.

## Open Questions

- Is `kenn-indexer`'s independence from `kenn-analyze` deliberate? This decides
  D2 (A vs B) and is answerable from the history of `post_aggregate_hook`.
- Should the per-language `communities` counter get the same treatment? It has the
  same raw-vs-earned ambiguity one level down, but no surface currently contrasts
  it with an earned number, so there is no active inconsistency to fix. Defer.
