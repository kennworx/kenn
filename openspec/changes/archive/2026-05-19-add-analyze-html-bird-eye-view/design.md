## Context

`kenn analyze` produces a markdown report (god-nodes, anchored hierarchy, flat cross-check). Readers need a visual artifact too: structure of relations, package coupling at a glance, drill-down at click distance. The brief is "bird's-eye of relations, scales to any repo size, no surprises."

The first attempts went through several JS graph libraries before settling: `vis.js` (vis-network) doesn't scale past ~5k nodes; `Sigma.js` needs `graphology-layout-forceatlas2` which isn't packaged as a working UMD; `Cytoscape.js`'s `cose`/`fcose` layouts hang on the 12k-node/121k-edge enterprise repo (~30 s blocking client-side layout); cosmos.gl in its default mode runs a continuous force simulation that the user found "constantly resizing/pulsing." The session converged on **Rust-precomputed static layout + cosmos.gl as a passive WebGL paint engine**.

Constraints carried in from the prior change (`add-graph-analysis-aggregation-and-hierarchy`):

- The aggregated graph artifact already lives in every snapshot — we have `nodes`, `edges`, `anchors` available via `Reader::scan_aggregate_*`.
- `kenn-analyze` is reader-only; no indexer changes.
- Per-workspace memory rules apply: no private project names hard-coded; renderer must be MIT (so the higher-level `@cosmograph/cosmograph` CC-BY-NC-4.0 bundle is out).

## Goals / Non-Goals

**Goals**:

- Single self-contained HTML file, no server, no build step. Double-click opens in a browser.
- Renders at all three scales we validate on: ~16 / ~1.6k / ~12k nodes.
- Static after first paint — no animation, no pulsing, no zoom-driven thickness changes.
- Bird's-eye view as the default mental model. Drill-down by click when graph is too dense to absorb in one shot.
- Per-workspace `<title>` so multiple `graph.html` tabs are distinguishable.
- Layout is byte-deterministic across runs.

**Non-Goals**:

- Live updates while indexing.
- 3D layouts.
- Editable graphs.
- Saving layout overrides (the layout is regenerated on every analyze).
- Replacing REPORT.md — both artifacts ship side-by-side.
- Custom node/edge styling beyond what kenn-analyze derives from snapshot data (kind, anchor, weight).

## Decisions

### Where layout runs: in Rust, once, deterministic

**Decision**: compute `(x, y)` for every node and every supernode in Rust at analyze time. Browser receives them as JSON and paints exactly what it's given.

**Why**: every client-side force simulator we tried either pulses (cosmos in default mode), hangs on the big repo (cytoscape cose/fcose), or has CDN packaging problems (Sigma + graphology-layout-forceatlas2). Server-side layout sidesteps every one of those failure modes and turns rendering into pure paint. We also get a deterministic-across-runs property for free.

**Alternative considered**: client-side `cytoscape-fcose` with progress reporting. Rejected on the 121k-edge repo where it stays at the layout step for ~30 s and the user can't tell whether it's hung.

### Layout algorithm: anchor disc packing + per-anchor Fermat spiral

**Decision**: each anchor gets a disc whose radius is `√node_count × NODE_AREA_UNIT` (clamped at `MIN_ANCHOR_RADIUS`). Discs are placed by a greedy sunflower-spiral pack — each anchor lands on the spiral at a cumulative radius derived from total prior area, then nudged outward until it doesn't overlap any previously placed disc (capped at MAX_NUDGES iterations per anchor). Within each disc, nodes land on a Fermat spiral (`r ∝ √(i/n)`, `θ = i × goldenAngle`), with a small inner blank ring so heavy nodes don't all converge to a single hub point.

**Why**: anchors as visually distinct islands match how a reader thinks about a codebase. Sunflower spirals give roughly even density without overlapping, in O(N) with a small per-anchor pack pass. The Fermat-spiral interior gives uniform-density packing and never produces visible row/column artifacts. The earlier attempt that placed heavy nodes at disc centers caused a "starburst hub" where all cross-anchor edges visually converged to one point — the blank inner ring fixes that.

**Alternative considered**: client-side iterative relaxation. Rejected per the prior decision — keeps the browser deterministic and dumb.

### Renderer: `@cosmos.gl/graph` (MIT), simulation disabled

**Decision**: load `@cosmos.gl/graph@2.6.4` from `https://esm.sh/` as a browser ES module. Configure with `disableSimulation: true`, `scalePointsOnZoom: false`, `scaleLinksOnZoom: false`, and feed it our pre-computed positions via `setPointPositions(Float32Array)`. Call `pause()` right after `render()` to belt-and-suspenders the no-simulation guarantee.

**Why**: cosmos's WebGL renderer handles 12k nodes / 121k edges trivially — that was the original draw. The earlier failure was *misuse*: it was running its force simulation continuously and we were also periodically calling `fitView` from a `setInterval`, which together produced the pulsing. With simulation off + positions authoritative, cosmos becomes a fast static painter.

**Alternative considered (and rejected)**:
- `vis.js` (`vis-network`): handles small graphs but stalls past ~5k nodes; doesn't WebGL.
- `Sigma.js` + `graphology-layout-forceatlas2`: no working UMD bundle; esm.sh transpilation broke Sigma's class hierarchy (`Cannot access E without 'new'`). Bundling locally with rollup adds a build step.
- `Cytoscape.js` with `name: 'preset'`: works at all scales because layout is preset, but its haystack edge renderer scales widths with zoom and the only way to keep widths constant is an O(E) zoom handler that lagged at 121k edges.
- Custom WebGL renderer (regl/picogl): would also work but ~400 LOC of shader and hit-test code we'd then own.

### License: only the MIT lower-level engine

**Decision**: load `@cosmos.gl/graph` (MIT). Do NOT load `@cosmograph/cosmograph` (CC-BY-NC-4.0, non-commercial).

**Why**: kenn is indexing private commercial repos at user sites. A CC-BY-NC dependency in our shipped output would prevent commercial use.

### Two display modes, picked by edge count

**Decision**: at startup, JS picks one of two initial modes:

- `≤ SUPERNODE_EDGE_THRESHOLD` (currently 5,000) → "all-detail": render every detail node and every detail edge.
- `>` threshold → "overview": render one supernode per anchor (sized by √node_count) + bundled anchor-to-anchor edges (summed weights per anchor pair). Click any supernode → "expanded:<anchor>": that anchor's disc of detail nodes appear in place, with intra-anchor detail edges; cross-anchor detail edges route to the OTHER anchor's still-visible supernode; the other anchors stay as supernodes. Click another supernode to swap. `Esc` returns to overview.

**Why**: the visual cost of rendering thousands of edges is fine for WebGL but the cognitive cost for the reader is not. The 5k threshold was chosen by trying the self-repo (~3.9k edges → all-detail still readable) and the enterprise repo (~121k edges → useless without bundling). Picking by edge count rather than node count matches what the reader actually sees on screen.

**Alternative considered**: always start in overview. Rejected — for tiny graphs like the TS workspace (16 nodes, 20 edges) the overview is just 5 supernodes with 3 bundled edges and the user has to click to see anything real. The threshold makes the default "right" at both ends.

### Bundled anchor edges, no per-edge bridge nodes

**Decision**: in expanded mode, when a detail node has a cross-anchor edge to a node in another (still-collapsed) anchor, render the edge as `detail_node → supernode_of_other_anchor`. The implementation rebuilds the link buffer on mode change rather than maintaining bridge edges across modes.

**Why**: keeps the steady-state data tight (no separate "bridge edge" set lives forever). Rebuilding on mode change is O(E_total) once, not O(E) per frame. Earlier attempt with `Cytoscape.edge.move()` per edge on every expand was O(E) with very high constants and hung the page at 121k edges; switching to "rebuild Float32Array link buffer + setLinks" is microseconds even at that scale.

### Streaming render: serde_json::to_writer instead of String → write

**Decision**: `html::render(graph, layout, workspace_name, &mut writer)` writes the HTML prefix, then `serde_json::to_writer(&mut *w, &payload)`, then the suffix — directly to a `BufWriter<File>`.

**Why**: the enterprise-repo HTML is ~8 MB. Materializing it as a `String` first means one ~8 MB allocation + a copy on write. Streaming bounds peak memory to whatever serde's row buffers are (a few KB).

### Click target: points only

**Decision**: cosmos's `onClick(idx)` fires only for points (idx is a number) or empty stage (idx is undefined). We never assign click semantics to edges.

**Why**: clicking edges is an accidental-click hazard in dense graphs. The user explicitly asked for points-only.

## Risks / Trade-offs

- **[Risk]** esm.sh availability: if `https://esm.sh/@cosmos.gl/graph` becomes unreachable, the HTML doesn't load at all. → **Mitigation**: the user can switch CDN by editing one URL in the generated file. Future option: vendor cosmos.gl alongside the binary and inline it.
- **[Risk]** Internet required: the generated HTML loads cosmos.gl from a CDN. → **Mitigation**: same as above — easy local-replacement path; downstream we may add an `--inline-deps` flag.
- **[Risk]** Anchor disc packing is O(N²) worst case (the greedy nudge inner loop). On a repo with 1000+ anchors this could measurably slow analyze. → **Mitigation**: real workspaces have <500 anchors; the enterprise repo with 366 anchors finishes packing in well under 100 ms. If a pathological case appears, swap the nudge loop for a quad-tree-accelerated pack.
- **[Risk]** Per-workspace title leaks the workspace directory name into the HTML. If the user opens it in a screen-share they expose that name. → **Mitigation**: the directory name is the same as the local path the user is working in; they're aware. The HTML is never published anywhere by kenn.
- **[Trade-off]** No support for editing layout — every `kenn analyze` regenerates positions deterministically. If a user pans/zooms and re-runs, they're back to the default view. Acceptable: the file is read-only by design.
- **[Trade-off]** Bundled anchor edges hide per-kind detail at overview. The hover panel shows total weight + edge count; the markdown REPORT.md still has the kind-aware breakdown. Acceptable: bird's-eye is for shape, drill-down is for detail.

## Migration Plan

- **Forward**: ship the change as a single release. The first `kenn analyze` after upgrade emits both files. No flags, no opt-in, no schema changes.
- **Rollback**: an older kenn binary continues to overwrite `kenn-out/graph.html` with whatever it produces (or doesn't touch it if it doesn't know about it). Stale HTML is harmless — it's a regenerated artifact.

## Open Questions

- Should the HTML offer a "save current camera + filter state" affordance (URL hash with `?focus=<anchor>` + zoom/pan)? Useful for sharing a specific view but adds JS complexity. Defer to a follow-up if requested.
- Worth inlining cosmos.gl into the HTML to eliminate the runtime CDN dependency? Cosmos.gl is ~190 KB minified. Doubles the smallest output (TS: 14 KB → ~200 KB) but the big files (enterprise: ~8 MB) barely move. Trade-off favors inline for offline use; defer until someone asks.
- Should kind-filter toggles be applied as a GPU-side color-alpha update instead of a buffer rebuild? Faster for big graphs but requires per-frame logic that complicates the otherwise pure paint model. Current rebuild-on-change is fast enough; revisit if it ever feels laggy.
