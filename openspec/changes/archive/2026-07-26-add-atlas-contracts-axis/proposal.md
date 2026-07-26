## Why

The atlas has two axes — **packages** (the dependency structure) and **domains**
(emergent Louvain clusters) — but neither answers a question every reader asks
when touching a shared abstraction: *"where is this interface implemented across
the tree?"* The `implements`/`extends_type` relationships that answer it are
explicit in the graph, yet the only place they surface today is as one relation
among many in a package's coupling table.

We tried to make the **domain** axis carry this by up-weighting is-a edges for
Louvain. It doesn't work: clustering yields an arbitrary, incomplete subset of
implementers (on `spf13/afero`, the `File` community pulled in one of three
backend packages and never the others, at any weight), the result is fragile
(single-edge cross-package merges sit on the modularity knife-edge), and the span
is capped by clustering, not by reality. The relationship is *explicit* — it
should be read directly, not rediscovered by an emergent algorithm.

## What Changes

- Add a third atlas axis, **contracts**: one concept per first-party interface /
  base class / protocol whose implementers span more than one package.
- Each contract concept lists **every** first-party, non-test implementer,
  grouped by package, read straight from the `implements` and `extends_type`
  aggregate edges — deterministic and complete, with no clustering.
- `index.md` gains a `## Contracts` section listing the broadest extension points
  (widest package span first), alongside `## Domains`.
- The bundle writes `contracts/<name>.md` files, mirroring `domains/<name>.md`.

Non-goals: no new edge kinds, no analysis/clustering change, no reindex
invalidation — this is a render-side projection of edges the aggregate graph
already stores.

## Capabilities

### New Capabilities
- `atlas-contracts`: the contracts axis — deriving, ranking, capping, and
  rendering cross-package interface→implementer relationships in the atlas bundle.

### Modified Capabilities
- `atlas-bundle`: the bundle's `index.md` header and section list, and the set of
  concept files it writes, gain the contracts axis. (Section addition only; the
  existing packages and domains axes are unchanged.)

## Impact

- **Code**: `crates/kenn-indexer/src/atlas/` — `model.rs` (new `ContractConcept` /
  `ContractImplementers`), `producer.rs` (`build_contracts`), `okf.rs`
  (`render_contract`, `contract_id`, `## Contracts` in `render_index`),
  `write_bundle`. The producer's `build_concepts` return and its callers gain the
  contracts vector.
- **Data**: reads existing `aggregate_edges` (`implements` kind 6, `extends_type`
  kind 17). No schema change.
- **Determinism / invalidation**: none — pure edge projection at render time, like
  the intra-domain hub ranking. No `kenn index --force` required to adopt it, and
  no generation bump.
- **Docs**: the `atlas` skill (`claude-plugins/kenn/skills/atlas/SKILL.md`) should
  describe the new axis so agents read it.
