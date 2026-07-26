## Why

`kenn domains` reports 11 domains for a snapshot whose atlas renders 10. Same
snapshot, same shared rule, different answers.

The extra one is real and reproducible on this repo:

```
$ kenn domains LlamaEmbedder      size 10 · spans kenn-embed(6) + kenn-store(4)

  rs:kenn-store::embed_one     → crates/kenn-store/examples/composed_spike.rs#385
  rs:kenn-store::embed_all     → crates/kenn-store/examples/prompt_ab.rs#88
  rs:kenn-store::embed_queries → crates/kenn-store/examples/fused_ab.rs#87
  rs:kenn-store::eval          → crates/kenn-store/examples/composed_spike.rs#516
```

All four of the members that earn the cross-package span are throwaway example
binaries. The producer drops example-path nodes before computing eligibility
(`producer.rs`, `is_example_path`), so it never renders this domain. The query
cannot: `AggregateNodeRow` carries no path, so it passes `example: false` into
the shared predicate and a bundled spike fabricates architecture.

The `atlas-axes-on-the-cli` change made producer and query share
`is_domain_eligible`. That was necessary and **not sufficient** — a shared
predicate still diverges when one caller cannot supply its inputs. Sharing the
rule while fabricating an argument to it is the same defect wearing a nicer
shape.

The same missing fact also skews `single_dominant`, which both surfaces compute
by counting eligible nodes per anchor.

## What Changes

- Example-path provenance SHALL become a persisted fact on the aggregate node,
  evaluated once at aggregation time, rather than a path join each consumer
  re-derives. **BREAKING** at the store-schema level; snapshots are regenerated
  by `kenn index`, never migrated.
- The atlas producer and the domains query SHALL both read that flag instead of
  computing (or fabricating) it, so neither can answer the eligibility question
  differently.
- The domain-eligibility rule SHALL name the example exclusion in the spec. It
  is load-bearing today and specified nowhere.

## Capabilities

### Modified Capabilities

- `index-store-db`: the `aggregate_nodes` table carries an `example` column
  beside `test` and `external` — the third provenance flag on a node.
- `atlas-bundle`: the domain-eligible set is defined to exclude example/sample/
  demo/fixture code, and both the producer and any query SHALL take that fact
  from the persisted node row rather than re-deriving it.

## Impact

The fact is already computable at the one place that should own it:
`aggregate::compute_and_persist` builds `files` (id → path) and
`primary_def_file` (symbol → file) for `resolve_anchors`, so evaluating
`is_example_path` there adds no scan and no traversal.

```
                         BEFORE                          AFTER

  aggregate_nodes    id kind name lang            id kind name lang
                     external test                external test example
                     anchor_id anchor_name        anchor_id anchor_name
                                                            │
  atlas producer     joins primary_def_file ──┐              │ reads the flag
                     → files → is_example_path │             │
                                               ├── same ─────┤
  domains query      cannot join;              │             │ reads the flag
                     passes `example: false` ──┘             │
                     ✗ DIVERGES                              ✓ one verdict
```

Touched: `kenn-model` (`AggregateNodeRecord`), `kenn-store` (DDL, `AggregateNodeRow`,
reader scan, writer insert, `STORE_SCHEMA_VERSION`), `kenn-indexer`
(`aggregate.rs` evaluates it, `atlas/producer.rs` consumes it), `kenn-mcp`
(`tools/domains.rs` stops fabricating it).

`is_example_path` keeps a second caller in `producer.rs` for intra-package
sub-areas, which ranges over all symbols rather than aggregate nodes; that call
site is unaffected.

Unblocks `honest-graph-counters` task 5.1 ("assert the `domains` stat equals what
a domains query returns"), which cannot pass while the query returns 11 and the
producer 10.
