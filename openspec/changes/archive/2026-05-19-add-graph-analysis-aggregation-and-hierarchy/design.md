## Context

The prototype on `main` proves the analysis pipeline (roll-up → projection → Louvain → REPORT.md) produces meaningful structural insights at the scales kenn targets: a multi-crate Rust workspace (~2k aggregate nodes), a ~70k-symbol enterprise C# repo (~12k aggregate / 93k undirected edges / 444 flat communities), and a TypeScript monorepo (small but well-formed). Three open weaknesses motivate this change:

1. **Recompute every run.** `kenn-analyze` re-projects the whole graph on each invocation. Iterating on cluster parameters (`--top-n`, future `--max-depth`, weight tuning) burns the same O(N + E) projection pass each time.
2. **No first-class aggregated artifact.** External callers (MCP today, other tooling later) that want to ask "which subsystem does this symbol belong to" or "what are the heaviest cross-subsystem edges" have to load the per-symbol graph and re-aggregate themselves.
3. **Flat clustering hides hierarchy.** A 600-node community that mixes a DbContext, services, and controllers is a real signal that there are 3–4 tight sub-clusters inside, but the current single-pass Louvain shows them only as one bag of names. A reader's mental TOC also organizes around packages/crates first, sub-systems second — the flat view doesn't match.

The change folds aggregation into the indexer pipeline (so the aggregated graph lives in every snapshot), shrinks `kenn-analyze` to a reader-only consumer, and lifts clustering into a two-view layered structure: an anchored hierarchy whose top level matches a reader's mental package model, plus a flat Louvain rendered alongside for cross-checking. Both views read from the persisted artifact; iteration on either is now bounded by clustering cost, not projection cost.

Ingest-time test detection (file globs + Rust descriptor heuristic) is already on `main` and is a prerequisite — the aggregation pass relies on `SymbolRecord.test` being populated.

## Goals / Non-Goals

**Goals:**

- Persist the aggregated graph as a first-class snapshot artifact. The redb tables live alongside symbols / edges / defs, ride snapshot lifecycle (rollback, GC), and are queryable through the existing `Reader` trait.
- Make the projection a one-time O(N + E) cost paid during `kenn index`, not per analyze run.
- Add anchored hierarchical Louvain whose top level matches packages / crates / top-level directories.
- Add flat Louvain over the same aggregated graph, rendered alongside the anchored hierarchy, with cross-anchor flags surfaced.
- Keep `kenn analyze` working on snapshots that pre-date the new tables (graceful fallback to in-memory recompute, warn once).
- Preserve every prototype invariant: same edge-kind weights, same enclosing-walk semantics, same live/test/external split for god nodes, same per-community test ratio.

**Non-Goals:**

- MCP tools that expose the aggregated graph (`community_of`, `community_path`, `aggregate_neighbors`, `query_graph`). Tracked as a follow-up change; the data shape this proposal lands is the prerequisite.
- Live/test split *inside* each community's member listing. Defer.
- Discovered-top-level as an opt-in alternative to package anchoring. The flat Louvain view already gives some of the same insight; a true `--top-level discovered` flag is a small follow-up once it's clear it's wanted.
- Populating `pkg` for Rust SCIP symbols. Tracked separately. This proposal handles the gap via the file-path fallback so it does not block.
- Replacing Louvain with Leiden. The current hand-rolled Louvain is good enough; switching is a separate quality-vs-dependency conversation.

## Decisions

### Where the aggregation pass runs

**Decision:** in the indexer pipeline, at end of `run_pipeline_with_progress`, after all per-unit ingest completes and before `end_run` finalizes the snapshot. A new `aggregate::compute_and_persist` step.

**Why:** the snapshot is the right unit of consistency. If aggregation lives outside it (separate command, separate file), rollback and GC have to learn about it; the on-disk state can disagree with the symbol tables after a partial run. Inside the snapshot, every existing lifecycle invariant applies for free.

**Alternative considered:** `kenn analyze` mutates the published snapshot to add aggregate tables on first access. Rejected: violates "snapshot is immutable after publish" — a property the lifecycle, MCP server, and rollback all rely on.

**Alternative considered:** aggregation streams during ingest (as edges arrive). Rejected for this change: the projection needs every symbol's `enclosing_symbol` resolved, which can cross documents (notably C# partial classes), and demands a deterministic order. End-of-run is simpler and the O(N + E) cost is small relative to the rest of ingest.

### Storage layout

**Decision:** two new redb tables, schema-versioned alongside the existing tables.

```
aggregate_nodes:  key = u32_be(aggregate_short_id)
                  value = bincode(AggregateNodeRecord {
                      kind: Kind,
                      name: String,
                      language: Language,
                      external: bool,
                      test: bool,
                      anchor_id: u32,           // package or fallback dir
                      anchor_name: String,
                  })

aggregate_edges:  key = pair_u32_be(min(src, tgt), max(src, tgt)) ++ u32_be(EdgeKind as u32)
                  value = u32_be(weight)
```

Endpoints are sorted at write time (undirected dedup). Per-kind weights are persisted separately so consumers can reweight without re-ingest. A separate `aggregate_meta` key in the existing `META` table records the schema version of the aggregate tables (lets us version this artifact independently of the overall snapshot version if we ever change the projection rules).

**Why:** matches the existing `db_default` backend conventions (key/value bincode pairs, big-endian u32 keys for range scans). Splitting nodes and edges into separate tables mirrors the symbol/edge split. Storing per-kind edges (rather than collapsing into one weight) keeps the door open for downstream consumers that want kind-aware analysis.

**Alternative considered:** single combined `aggregate` table with one row per node carrying its full adjacency list. Rejected: poor for range queries, awkward when only a subset of kinds matter.

### Anchor source

**Decision:** for each aggregate node, the anchor is determined by the following lookup chain, in order:
1. The symbol's `pkg` field if non-zero.
2. The first path component (workspace-relative, forward-slash-separated) of the symbol's primary def file.
3. Literal `"<unanchored>"` for symbols with neither.

The anchor's stable id is interned at aggregation time (small string → u32) and persisted on the aggregate node.

**Why:** `pkg` is the canonical answer when the indexer populates it (currently C# JSONL path only). The path-prefix fallback is robust today for every language and produces strings that match a reader's mental crate/project label without any new wiring. Once the Rust SCIP path starts populating `pkg`, the same nodes will start using the package id without any data migration — the anchor *name* may change for those nodes between snapshots, which is acceptable.

**Alternative considered:** require `pkg` to be populated for every language as a prerequisite. Rejected: that's a separate, non-trivial change and would block this proposal on it for no real benefit.

### Hierarchical clustering algorithm

**Decision:** recursive Louvain. The outer step assigns L0 = anchor (no clustering, just a partition by anchor id). Within each L0 group, run the existing single-level Louvain on the induced subgraph. Each resulting community at L1 with ≥ `min_cluster` nodes is recursed into for L2, and so on, up to `max_depth`.

Pseudo:

```
fn hierarchical(graph, anchor_of, max_depth, min_cluster) -> Hierarchy {
    let mut tree = Hierarchy::root();
    for (anchor, members) in partition_by_anchor(graph, anchor_of) {
        let subgraph = induced_subgraph(graph, &members);
        let child = recurse(subgraph, depth=1, max_depth, min_cluster);
        tree.add_anchor(anchor, child);
    }
    tree
}

fn recurse(subgraph, depth, max_depth, min_cluster) -> Subtree {
    if depth >= max_depth || subgraph.node_count() < min_cluster {
        return Subtree::leaf(subgraph.nodes());
    }
    let partition = louvain(subgraph);   // existing impl
    let mut node = Subtree::internal();
    for community in partition {
        if community.len() < min_cluster {
            node.add(Subtree::leaf(community));
        } else {
            let sub = induced_subgraph(subgraph, &community);
            node.add(recurse(sub, depth + 1, max_depth, min_cluster));
        }
    }
    node
}
```

Determinism: every step sorts inputs by `ShortId` and uses the prototype's deterministic Louvain implementation. Level ids are assigned in the deterministic visit order (anchor name asc, then community size desc, then min member id asc).

**Why:** smallest delta from the prototype. The current `cluster::louvain` already produces a deterministic flat partition; reusing it inside a recursion gives a hierarchy with no algorithmic surprises and one new code path (the recursion driver) rather than swapping the algorithm.

**Alternative considered:** multi-level Louvain (the "real" version that coarsens nodes into super-nodes and re-runs). Equivalent in spirit but harder to reason about — the prototype's single-level Louvain is the building block we already trust, and induced subgraphs are easier to debug than coarsened graphs.

**Alternative considered:** Leiden (graspologic / `rustworkx-core`). Higher quality on some graphs but pulls a dep. Defer until we have evidence the quality matters at our scale.

### Flat Louvain alongside

**Decision:** run the existing flat Louvain over the whole aggregated graph in parallel with the anchored hierarchy. Each flat community gets a `spans_anchors: Vec<AnchorId>` field populated at render time. The report renders flat communities under a "## Flat Communities (cross-check)" heading, with cross-anchor ones flagged.

**Why:** the cross-check is one of the most useful signals the layered view enables — communities that cut across anchors point at concepts that span crates (refactor candidates or intentional design glue). The cost is one extra Louvain pass on an in-memory graph; negligible.

### `kenn-analyze` as reader-only

**Decision:** the aggregation logic moves wholesale to `kenn-indexer`. `kenn-analyze` shrinks to (1) read aggregate tables via `Reader::scan_aggregate_*`, (2) run clustering (hierarchical + flat), (3) render `REPORT.md`. The existing on-the-fly projection becomes the fallback path, triggered when the aggregate tables are missing or carry an unknown schema version.

**Why:** clean separation of concerns. The indexer owns ingest; analyze owns presentation. Fallback keeps `kenn analyze` working on older snapshots without forcing a re-index.

### Schema version bump

**Decision:** bump `SCHEMA_VERSION` in `kenn-store/src/backends/db_default/schema.rs` from `1` → `2`. Snapshots at version 1 are still readable by the reader (no schema breakage for existing tables); the `Reader::scan_aggregate_*` methods return empty on a v1 snapshot, and `kenn-analyze` detects this and falls back.

**Why:** the aggregate tables are a *new* artifact in the snapshot, not a change to existing ones. A version bump documents the addition; the absence of the tables on older snapshots is the version signal in practice.

## Risks / Trade-offs

- **[Risk] Anchor labels drift when `pkg` later gets populated for Rust.** A node that today shows anchor `"crates/kenn-indexer"` (from the path fallback) will later show anchor `"kenn-indexer"` (from `pkg`). Reports cited in commit messages or PRs will name the old label. → **Mitigation:** the difference is cosmetic. Both labels point at the same set of nodes, and the report header makes the source of the anchor (`pkg` vs path) visible to a reader.

- **[Risk] Aggregated graph desynchronizes with symbol tables if `end_run` partially fails.** End-of-run write order matters: if symbols are committed but aggregate writes fail, the snapshot has inconsistent data. → **Mitigation:** aggregate writes happen inside the same `end_run` boundary as the rest. Lifecycle already treats partial publish as "do not flip live"; if aggregation fails, the snapshot is not published and the previous live snapshot stays. Worst case: the user sees a failed `kenn index` and reruns.

- **[Risk] Hierarchical depth blows up on pathological graphs.** A dense, highly modular subgraph could in theory recurse many levels deep. → **Mitigation:** `max_depth` (default 4) hard-caps; nothing recurses below `min_cluster` (default 20). Both are user-tunable via CLI flags.

- **[Risk] Two clustering passes (hierarchical + flat) double clustering cost.** → **Mitigation:** clustering runs on the in-memory aggregated graph and is bounded by `O(E · iter)`; at the scales we target (max ~100k aggregate edges) both passes together complete in well under a second. The expensive part (projection) now only runs once per snapshot.

- **[Trade-off] `kenn-analyze`'s public Rust API changes.** `cluster::louvain` now returns `Hierarchy` instead of `Vec<Vec<ShortId>>`. → **Mitigation:** there is one in-tree caller (the CLI command) and no published consumers. Expose `Hierarchy::flat()` for ergonomic access to the flat view.

- **[Trade-off] Two views in one report means more to scan for the reader.** The anchored hierarchy is the primary view; the flat cross-check is a small second section. Most days the reader will skim flat for cross-anchor flags and spend their time in the hierarchy.

- **[Trade-off] Determinism requires sorted iteration everywhere — slight CPU cost over a `HashMap` walk.** Worth it for reproducibility across reindexes; this is a property the prototype already maintains.

## Migration Plan

- **Forward:** ship the change as a single release. Re-indexing populates the aggregate tables; existing snapshots stay readable.
- **Fallback path on pre-aggregate snapshots:** `kenn-analyze` detects empty `aggregate_nodes`, recomputes in-memory (current prototype path), and prints a single-line warning suggesting `kenn index --force`.
- **Rollback** (snapshot-level): rolling back to a snapshot that pre-dates this change works; `kenn analyze` against that snapshot uses the fallback path.
- **Rollback** (binary-level): a kenn binary built before this change reading a snapshot that includes the aggregate tables ignores them (`open_table` returns `TableDoesNotExist` for unknown names is not the case in redb — unknown tables simply aren't opened). The pre-change binary doesn't read aggregate tables, so the extra data is silent.

## Open Questions

- **Anchor naming for path fallback** — first path component is robust but coarse on flat repos (everything ends up under `src/`). Worth a heuristic that walks one or two segments deeper when the first component is generic (`src`, `lib`, `source`)?
- **Per-anchor minimum size for recursion** — should very small anchors (< 5 nodes) still appear in the hierarchy, or be folded into a synthetic "Misc" anchor? Tentative: render them as flat leaves under their own anchor heading. Revisit after first reports.
- **Edge cross-check granularity** — when a flat community spans 3 anchors, do we list all three in the report row, or just count? Tentative: list up to 3, then "+N more".
