## Why

`kenn overview` reports `cross_anchor_communities: 38` for a snapshot whose atlas
renders **9** domains. Both numbers describe "cross-package clusters" and they
disagree by 4x, with nothing on either surface saying which is which.

The 38 is the raw flat-Louvain count: every community that happens to touch more
than one anchor. The 9 are the domains that clear the axis floors — a package
joins a domain's span only with enough members AND a first-party edge to another
such package. Those floors exist because raw communities systematically overstate:
they include packages joined only through a shared vendor type, plus one-symbol
stragglers. The atlas was changed to stop publishing the raw number precisely
because it was misleading; the overview still publishes it, unlabelled.

A reader comparing the CLI to the generated atlas cannot tell which surface lied.
That is the failure mode the package-coupling work was done to prevent, recurring
on a counter instead of a threshold.

## What Changes

- The workspace overview's graph summary SHALL distinguish the RAW clustering
  counter from the EARNED domain count, each named for what it is, so neither can
  be mistaken for the other. **BREAKING** for any consumer that reads
  `cross_anchor_communities` as "the number of domains".
- The earned count SHALL be computed at index time and persisted as a `stats`
  row, because the overview is specified to do no aggregation on the read path.
- Resolve where the earned-span rule lives so that whichever component writes the
  stat row can reach it (see design — this is the substance of the change).

## Capabilities

### Modified Capabilities

- `mcp-server`: the workspace overview's whole-graph summary reports both the raw
  cross-anchor community counter and the earned domain count, each named for what
  it is; the raw counter keeps its existing meaning.
- `graph-analysis`: the earned domain count is recorded as a whole-graph stat row
  alongside the existing `hierarchy_depth` / `cross_anchor_communities` counters.
  Written by the AGGREGATION stage, not the analysis pass — see design D2: the
  earned-span rule lives in `kenn-indexer`, and `kenn-analyze` is forbidden from
  depending on it, so the aggregation stage reads the persisted communities back
  and writes the row. The analysis pass's existing counters are unchanged apart
  from `cross_anchor_communities` being documented as the RAW diagnostic.

## Impact

The blocking question is **where the earned-span rule can live**, because today no
component that writes graph stats can reach it:

```
kenn-analyze                         kenn-indexer/atlas
  writes graph stat rows               owns the earned-span rule
  graph_stat_rows(nodes, recs)         build_domains(… edges …)
  ✘ no edges parameter                 ✘ Option<AtlasContext> — may not run
                    ✘ neither crate depends on the other ✘
     (both are siblings over kenn-model / kenn-config / kenn-store;
      the analysis pass reaches kenn-indexer only via an injected hook)
```

Candidate resolutions, to be decided in design:

- Move the earned-span rule into `kenn-analyze` (it is arguably a graph-analysis
  concern — a stricter form of the `cross_anchor` predicate it already computes)
  and have the atlas consume it. Adds a `kenn-indexer → kenn-analyze` dependency.
- Put the rule in `kenn-model`, which every crate already depends on. Zero new
  dependencies, but places selection policy in a data-model crate.
- Have the atlas producer write the stat row. Cheapest, but the counter then
  exists only when the atlas ran.

Depends on `atlas-axes-on-the-cli` only for the rule extraction (that change moves
the earned-span logic out of `producer.rs` into a shared module); this change
decides which crate that module ultimately belongs to. It can be designed in
parallel and applied after.
