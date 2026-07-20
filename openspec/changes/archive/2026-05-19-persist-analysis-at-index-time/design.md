## Context

`kenn analyze` today is a per-invocation pipeline: it loads the aggregate graph from the snapshot, runs Louvain hierarchical + flat clustering and god-node ranking in memory, and emits REPORT.md (+ optionally graph.html). Nothing is persisted. Three problems flow from that:

1. **No reader-friendly substrate for the analysis.** The clusters and god-nodes are first-class facts about the codebase (which package owns this symbol? what are the bottleneck classes?), but every reader currently has to recompute them. `kenn analyze` on a 12k-node workspace costs 200 ms of CPU per invocation in release, multi-second in debug — fine once, terrible repeated. Persisting them means future consumers (a follow-up MCP proposal first) can read instantly.
2. **The "report" and "visualization" concerns are tangled.** REPORT.md is a textual rollup of the analysis; graph.html is a spatial rendering of the aggregate graph plus a layout pass. They share the analysis compute but their outputs are independent. Conflating them under a single `kenn analyze` command means a user who wants the report has to know about `--graph`, and a user who wants the HTML has to wait for Louvain regardless.
3. **No single source of truth for community membership.** If a future tool wants to ask "what flat-Louvain community is symbol X in?" it has to recompute the entire clustering — which is non-deterministic across versions of the algorithm — to answer. Persisting the result pins membership at the snapshot.

Constraints carried in:

- Storage is the existing embedded-DB layer with the `Reader`/`Writer` API. New tables follow the same `*Row` / `*Record` convention as `AggregateNodeRow`, `AggregateEdgeRow`.
- Determinism: snapshot-to-snapshot, identical input must produce identical analysis output (Louvain ordering already deterministic in `kenn_analyze::cluster`).
- Per-workspace memory rule: no private project names in code, docs, or shareable output.
- The renamed command must be self-explanatory for someone who hasn't read the change log — `kenn visualize` reads.

## Goals / Non-Goals

**Goals:**

- Eliminate analysis recompute on every `kenn analyze` invocation by persisting the output of `cluster::hierarchical`, `cluster::louvain_flat`, and `top_by_weighted_degree` into snapshot tables.
- Move REPORT.md emission into the index pipeline so the report ships with the snapshot.
- Reduce the visualize command to a pure reader + layout + writer.
- Make both writers gated by config so heavyweight steps can be skipped on resource-constrained indexing runs.
- Land the storage substrate in a shape a follow-up MCP proposal can read from without further schema work.

**Non-Goals:**

- Exposing the persisted analysis via MCP. The tools (`list_god_nodes`, `get_communities`, `get_community_for_symbol`, anchor listing) belong to a follow-up proposal that depends on this change's storage shape.
- Recomputing or refining the analysis incrementally — the snapshot is rebuilt as a whole on `kenn index`; this change does not introduce partial updates.
- Adding a per-symbol "hot path" community lookup beyond the simple `symbol_id → community_id` map. Richer queries (community membership at arbitrary hierarchy depth, neighborhood walks) are out of scope.
- Changing Louvain itself. Same algorithm, same determinism.
- Persisting layout positions. The layout pass remains a `kenn visualize` concern; positions are not snapshot data.
- A 1.0 migration tool for old snapshots — users re-index with `--force`.

## Decisions

### Persist as four narrow tables, not a single JSON blob

**Decision:** add four typed tables: `analysis_god_nodes`, `analysis_flat_communities`, `analysis_anchored_hierarchy`, `analysis_node_membership`. Each has its own `Row`/`Record` types in `kenn-store`, with `scan_*` readers and a batched write step.

**Why:** mirrors the existing schema pattern (`aggregate_nodes` + `aggregate_edges`), gets typed deserialization for free, allows column-pruned scans for future readers (the follow-up MCP proposal will want filtered slices like "top god-nodes for filter=live"), and surface-level diffability is better than opaque blobs. The community-member lists fit naturally as separate rows keyed by community id.

**Alternative considered:** a single `analysis_blob` row per snapshot holding a serialized `AnalysisResult`. Smaller code change but worse for partial reads (filtered queries would still pay full deserialization), and any schema evolution becomes ad-hoc rather than first-class.

### Index-time analysis runs after aggregation, in the same pipeline

**Decision:** the indexer's existing `workflow::index_workspace` gains a new phase after `phase_aggregate` (or whatever the current step is named): `phase_analyze`. It calls `kenn_analyze::compute_analysis(&graph, &opts)` and writes the resulting `AnalysisResult` through the new store writer methods. Report rendering happens here too when `[index] write_report` is true.

**Why:** keeps the analysis owned by the same process that owns aggregation. Avoids the awkward "MCP starts indexing in the background; the user runs `kenn analyze` manually too" coordination problem. The pipeline already has phase-boundary error handling, progress reporting, and snapshot transaction semantics we want to reuse.

**Alternative considered:** a separate `kenn analyze --persist` flag that the indexer invokes via `cmd::analyze::run` after indexing. Rejected — it crosses the CLI boundary for what is internal pipeline work.

### Split `kenn_analyze::analyze` into `compute_analysis` + `render_report`

**Decision:** the existing `analyze()` async function (which mixes load + compute + write) is replaced by two pure functions:

```rust
pub struct AnalysisResult {
    pub anchors: AnchorMap,
    pub hierarchy: HierarchyTree,
    pub flat: Vec<FlatCommunity>,
    pub god_live: Vec<RankedNode>,
    pub god_test: Vec<RankedNode>,
    pub god_external: Vec<RankedNode>,
}

pub fn compute_analysis(graph: &AggregatedGraph, opts: &AnalysisOptions) -> AnalysisResult;
pub fn render_report(graph: &AggregatedGraph, result: &AnalysisResult) -> String;
```

The indexer consumes both, then hands `AnalysisResult` to the new persistence layer. `kenn visualize` consumes only `compute_analysis` (well, actually it reads the persisted form; see next decision) and `layout::compute` + `graph::render`.

**Why:** pure functions are easy to call from multiple call sites (indexer, visualize, tests) and the data flow is explicit. The current `analyze()` does IO inside the compute path — splitting it keeps the compute deterministic.

### `kenn visualize` reads from the snapshot, not the graph in memory

**Decision:** `kenn visualize` opens the snapshot for read, calls `Reader::scan_aggregate_*` for the graph (same as today), and calls `Reader::scan_analysis_*` for clusters/god-nodes. It does NOT call `compute_analysis` — that path is dead for the CLI command.

**Why:** the whole point is to avoid recompute. If the user wants to regenerate analysis they `kenn index --force`. This makes visualize cheap (<100 ms even in debug for the 12k-node case) and removes the duplicate-compute trap.

**Edge case:** snapshot lacks analysis tables (pre-this-change or `[index] persist_analysis = false`). Visualize errors out with the same "run `kenn index --force`" message the existing missing-aggregate guard uses. The exit code is `Generic`, matching today's missing-aggregate behavior.

### Config-controlled writes

**Decision:** add `[index]` with two booleans:

```toml
[index]
write_report = true
persist_analysis = true
```

Both default to true. Setting `persist_analysis = false` skips the analysis compute entirely (no point writing the report either if analysis was skipped; we make `write_report` imply `persist_analysis` by treating the latter as required when the former is true).

**Why:** indexing budgets vary. A CI job that just needs `kenn index` to populate the symbol DB for search shouldn't be forced to pay the Louvain cost. The default true matches the "include everything" stance of the existing tool.

### Rename, don't keep an alias

**Decision:** `kenn analyze` is gone; only `kenn visualize` exists. `[analyze]` becomes `[visualize]`. The `--graph` flag becomes `--algo` on visualize (and now spectral/force/stress/linlog are the only behaviour — no "skip graph emission" mode since visualize's only output is graph.html).

**Why:** keeping both names doubles the API surface forever. The change is small, breaking now is cheaper than supporting two names for two years. The starter kenn.toml is the natural place users will discover the new section name.

**Alternative considered:** keep `kenn analyze` as a hidden alias for `kenn visualize`. Rejected — the semantics are different (the old `analyze` wrote REPORT.md, the new `visualize` doesn't), so an alias that quietly does something different would be more confusing than removal.

## Risks / Trade-offs

- **[Risk]** Index-time analysis lengthens the `kenn index` wall by ~150 ms (release) on big workspaces. **Mitigation:** the `[index] persist_analysis = false` opt-out lets users on tight indexing budgets skip it. CI users typically need the analysis less than interactive users.
- **[Risk]** Schema migration: snapshots produced before this change have no analysis tables, so `kenn visualize` will error out until the user re-indexes. **Mitigation:** clear error message pointing at `kenn index --force`. Same UX as the existing missing-aggregate guard, which users have already encountered.
- **[Risk]** The `--graph` rename to `--algo` may surprise users who scripted the flag. **Mitigation:** breaking change is called out in the proposal; the prior change set was small (this is the first downstream consumer of the previous CLI).
- **[Trade-off]** Persisting analysis pins Louvain output to the snapshot. If we improve the algorithm later, old snapshots keep their old results until re-indexed. Acceptable — that's the point of snapshots; "kenn index --force" is a one-line user action.
- **[Trade-off]** Four new tables grow the snapshot by <1 MB on a 12k-node workspace, but big-monorepo budgets need to be revisited if that grows non-linearly. **Mitigation:** the new tables are sized by `O(anchors + communities + symbols)` — sub-linear in edges, so growth is bounded.
- **[Trade-off]** Renaming `cmd_analyze.rs` and dropping `kenn analyze` invalidates any downstream tools, scripts, or docs that reference the old name. Acceptable given low downstream count today.

## Migration Plan

- **Forward:** ship the change as a single release. First `kenn index` after upgrade writes the analysis tables and REPORT.md as part of the index step. `kenn analyze` no longer exists — `kenn visualize` is the replacement.
- **Rollback:** an older `kenn` binary continues to read the snapshot for everything except the new analysis tables (which it doesn't know about). It will keep running its own per-invocation `kenn analyze` if a user explicitly invokes the old command. The new tables are harmless extra data.
- **Follow-up:** the next change proposal will expose the persisted analysis via MCP read tools. No further schema work needed there — this change lands the substrate.

## Open Questions

- Should `analysis_node_membership` carry the full anchored-hierarchy path (root → leaf), or just the leaf community id, with path computed on demand? Path is small (typical depth ≤ 4) but redundancy is cheap; leaning toward storing the leaf only and reconstructing the path via a parent-pointer scan when asked. Resolve during implementation.
- Should `[index] write_report` accept a `path` override or always go to `<workspace>/kenn-out/REPORT.md`? Keeping it boolean for now; if someone asks for a path override later it's a small additive change.
- `kenn visualize` doesn't currently surface the analysis it reads. Should the JSON payload in `graph.html` include flat-community / god-node tags so the browser-side UI can show them? Defer until someone wants the surface in the visualization.
