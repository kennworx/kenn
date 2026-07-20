## ADDED Requirements

The "aggregated graph" referenced below is the weighted undirected graph
defined in the `graph-analysis` capability: a projection of the per-symbol
graph in which each method, field, free function, parameter, etc. is
rolled up to its nearest enclosing class-like or module-like symbol, and
edges of the kept kinds (`calls`, `type_use`, `field_access`, `implements`,
`instantiates`, `overrides`, plus module-to-module `imports`) are
aggregated as undirected weighted edges between those anchor symbols.
See `specs/graph-analysis/spec.md` for the full roll-up rules, kept kinds,
and per-kind weights.

### Requirement: Aggregate-graph computation during end_run

The indexer pipeline SHALL compute the aggregated graph as a step inside `end_run`, after every per-unit transform has flushed its symbol, edge, and def records, and before snapshot publication. Both the SCIP transform path and the JSONL transform path feed into the same aggregation step — aggregation reads the already-persisted symbol and edge tables to perform the roll-up rather than re-deriving aggregates per document.

The aggregation step SHALL:

1. Build an in-memory `HashMap<ShortId, SymbolRow>` by streaming the symbol table.
2. Compute `aggregate_id` for each symbol by walking the `enclosing_symbol` chain to the nearest class-like or module-like symbol (cycle-safe; falls back to self when no anchor is found).
3. Stream every persisted edge, look up the aggregates for both endpoints, drop self-loops on the aggregate graph, drop kinds not in the kept-kinds set, and accumulate weights into a `HashMap<(min_agg, max_agg, EdgeKind), u32>`.
4. Resolve each aggregate node's anchor via `pkg` → file-path prefix → `<unanchored>`.
5. Persist the resulting nodes and edges to the new `aggregate_nodes` / `aggregate_edges` tables.

#### Scenario: Aggregation runs once per index, not per document

- **WHEN** a workspace with N source documents is indexed
- **THEN** the aggregation step MUST execute exactly once per `kenn index` invocation, after all N documents are ingested

#### Scenario: Aggregation reads from persisted tables, not from in-flight buffers

- **WHEN** the aggregation step begins
- **THEN** it MUST source symbols and edges from the snapshot's persisted tables (via the same `scan_*` paths the analyzer uses)
- **AND** it MUST NOT depend on transform-time per-document state

### Requirement: Aggregation cost budget

The aggregation step SHALL be O(N + E) in the number of symbols and persisted edges. On a typical workspace its contribution to total `kenn index` wall-time SHALL be under 10%. Compliance is measured via the existing `KENN_BENCH` instrumentation by adding a `BENCH end_run: aggregate=<ms>` line to the pipeline output.

#### Scenario: Bench output reports aggregate timing

- **WHEN** `KENN_BENCH=1 kenn index` runs against any workspace
- **THEN** the bench output MUST include a line of the form `BENCH end_run: aggregate=<integer>ms`

### Requirement: Aggregation determinism

The aggregation step SHALL be deterministic: indexing the same source state twice MUST produce byte-identical `aggregate_nodes` and `aggregate_edges` tables. Iteration orders MUST be sorted (symbols by `short_id` ascending; edges by `(min_agg, max_agg, kind)`).

#### Scenario: Repeated index produces identical aggregate tables

- **WHEN** `kenn index --force` runs twice in succession on an unchanged workspace
- **THEN** both runs MUST produce snapshots whose `aggregate_nodes` and `aggregate_edges` tables, when scanned, return identical byte sequences

### Requirement: Aggregation tolerates incomplete ingest

When `kenn index` reports a `partial` status (at least one unit failed), aggregation SHALL still run on the symbols and edges that did ingest successfully. The snapshot SHALL be published as `partial` with non-empty aggregate tables reflecting whatever was ingested.

#### Scenario: Partial ingest still produces aggregated graph

- **WHEN** one of three configured C# projects fails during ingest but the other two succeed
- **THEN** the published snapshot's `aggregate_nodes` and `aggregate_edges` tables MUST contain the rolled-up graph of the two successful projects
- **AND** the snapshot status MUST be `partial`
