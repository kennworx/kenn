## 1. Layout in Rust (kenn-analyze)

- [x] 1.1 Add `crates/kenn-analyze/src/layout.rs` with `Layout { positions: Vec<(ShortId, f32, f32)>, anchor_discs: Vec<(String, f32, f32, f32)> }`.
- [x] 1.2 Implement `pub fn compute(graph: &AggregatedGraph, anchors: &AnchorMap, algo: LayoutAlgo) -> Layout` — coupling-aware anchor placement (spectral seed via deflated power iteration on the random-walk smoothing matrix, or sunflower seed for the disconnected case) followed by an algorithm-specific refinement pass and a non-overlap cleanup; Fermat-spiral per-anchor node placement; deterministic iteration (anchors by size desc/name asc, nodes by weighted-degree desc/short_id asc).
- [x] 1.3 Unit tests covering: determinism across two calls, every node placed exactly once, anchor discs do not overlap each other, empty graph yields empty layout.
- [x] 1.4 Expose `layout` module and `LayoutAlgo` enum (`Spectral` | `Force` | `Stress` | `LinLog`, with `LayoutAlgo::parse(&str)`) from `crates/kenn-analyze/src/lib.rs`.
- [x] 1.5 Implement `force_layout` — Fruchterman-Reingold with per-anchor-normalized spring attraction, `k²/d` repulsion, linear cooling, Hooke-like gravity toward origin, and a hard canvas-radius clamp.
- [x] 1.6 Implement `stress_layout` — all-pairs Dijkstra on the anchor super-graph (`1/√weight` edge lengths) followed by iterative stress majorization against graph-metric target distances.
- [x] 1.7 Implement `linlog_layout` — Noack's LinLog model with constant per-edge attraction and logarithmic repulsion, seeded from the spectral embedding.

## 2. AnalyzeOptions / AnalyzeReport surface

- [x] 2.1 Add `workspace_name: String` and `graph_layout: Option<LayoutAlgo>` fields to `AnalyzeOptions`; defaults via `Default`.
- [x] 2.2 Add `graph_path: Option<PathBuf>` to `AnalyzeReport` (`Some(path)` only when `graph_layout` was set).
- [x] 2.3 `kenn_analyze::analyze()` errors out when the aggregate tables are missing; when `graph_layout` is `Some`, it computes the layout once, opens a `BufWriter<File>` at `<out>/graph.html`, and calls `graph::render(&graph, &positions, &opts.workspace_name, &mut writer)`.
- [x] 2.4 Wire `serde` + `serde_json` into `crates/kenn-analyze/Cargo.toml` and add `kenn-config` as a dependency.

## 3. HTML renderer (kenn-analyze::graph)

- [x] 3.1 Module `crates/kenn-analyze/src/graph.rs` (renamed from the original `html.rs`) exposes `pub fn render<W: Write>(graph: &AggregatedGraph, layout: &Layout, workspace_name: &str, w: &mut W) -> io::Result<()>` — streams prefix, `serde_json::to_writer(payload)`, suffix; no full-string materialization.
- [x] 3.2 JSON payload shape `{ nodes: [{id, name, kind, language, anchor, external, test, weight, x, y}], edges: [{a, b, kind, weight}], supernodes: [{anchor, node_count, total_weight, x, y, radius}], anchor_edges: [{a, b, weight, count}], anchors: [...], kinds: [...] }`. Iteration orders sorted for byte-determinism.
- [x] 3.3 Compute `supernodes` from the layout's `anchor_discs` + per-anchor weight/count rollup of `nodes`.
- [x] 3.4 Compute `anchor_edges` by grouping graph edges by sorted (`min_anchor`, `max_anchor`); skip intra-anchor edges; sum weights and count distinct underlying edges.
- [x] 3.5 Title substitution: replace `{{TITLE}}` in template with `kenn analyze — <workspace_name>` (or generic when empty), HTML-escaped. The sidebar `<h1>` reuses the same `{{TITLE}}` placeholder so it mirrors the page title.
- [x] 3.6 Store the HTML/JS template in `crates/kenn-analyze/src/graph.html` (separate file) and load via `include_str!`; replace `{{DATA_JSON}}` placeholder via `split_template()` lookup so the payload streams into the gap.

## 4. Browser-side renderer (cosmos.gl, static)

- [x] 4.1 Import `Graph` from `https://esm.sh/@cosmos.gl/graph@2.6.4` as a `<script type="module">` (NO `@cosmograph/cosmograph` — license).
- [x] 4.2 Configure cosmos with `disableSimulation: true`, `scalePointsOnZoom: false`, `scaleLinksOnZoom: false`, `pointGreyoutOpacity ≈ 0.06`, `enableDrag: false`, `fitViewOnInit: true`, and call `graph.pause()` right after `graph.render()` on first frame.
- [x] 4.3 Two view-builder functions: `buildOverviewView()` (supernodes + bundled edges, sized by √node_count) and `buildDetailViewFiltered(focusAnchor | null)` (all detail when null; focused anchor + other-anchor supernodes + intra+cross-anchor edges when set).
- [x] 4.4 `switchView(view)` clears any active selection (state + `graph.unselectPoints()`), then wires all five cosmos arrays (`setPointPositions`, `setPointColors`, `setPointSizes`, `setLinks` with interleaved `[src0, tgt0, src1, tgt1, ...]` Float32Array, `setLinkColors`, `setLinkWidths`), renders, pauses, fits.
- [x] 4.5 Initial-mode pick: `DATA.edges.length > SUPERNODE_EDGE_THRESHOLD (5000)` → overview; else all-detail.

## 5. Selection UX

- [x] 5.1 Implement a selection stack (one-deep): clicking unselected node pushes selection; clicking the currently-selected node pops and restores the previous if any; second click on the now-restored node clears.
- [x] 5.2 Use `graph.selectPointsByIndices([focus, ...neighbors])` to focus the 1-hop neighborhood of the active selection; rest dims via `pointGreyoutOpacity` and the link-greyout default.
- [x] 5.3 `Esc` keydown handler: clear current selection AND clear the one-deep stack in one press; if no selection but an anchor is expanded, `Esc` returns to overview.
- [x] 5.4 Clicks on edges/stage MUST NOT change selection (cosmos's `onClick(idx)` only fires for points; ensure we early-return when idx is undefined).
- [x] 5.5 Clicking an anchor entry in the sidebar legend toggles expansion into that anchor (or collapses back if it's already focused).

## 6. Selected-node info pane

- [x] 6.1 Add a `#selection-info` element inside a `#top-left` flex column container (alongside `#mode-bar`) so the panes stack without manual offset. Mirror with a `#top-right` flex container around `#hover`.
- [x] 6.2 Render name (color = anchor color), kind · language, anchor (bold, colored swatch), weighted degree, external/test tags into the pane on every selection change.
- [x] 6.3 Hide the pane whenever the selection is cleared (Esc, click-selected-twice from empty stack, mode switch, stage click).

## 7. Sidebar + filters

- [x] 7.1 Populate counts (`<DATA.nodes.length> nodes · <edges.length> edges · <anchors.length> anchors`).
- [x] 7.2 Per-kind edge toggle checkboxes (one per kind that appears in the data, each with a colored swatch matching the kind palette); unchecking a kind drops those edges from the active view.
- [x] 7.3 `hide external packages` + `hide test code` filter checkboxes. Both filter detail nodes; both also drop overview supernodes (and their bundled edges) whose member nodes are entirely filtered out.
- [x] 7.4 Search input — debounced (~120ms); matches node name (or anchor name in overview).
- [x] 7.5 Anchor legend — anchor name with colored swatch + node count, sorted by node count desc; entries are clickable (task 5.5). Anchor list is the only scrollable area in the sidebar (flex layout with thin custom scrollbar).
- [x] 7.6 Any filter / search change triggers `rebuildCurrentView()` which re-runs the active view builder; filtered-out endpoints never coerce to point 0.
- [x] 7.7 Draggable splitter (`#splitter`) between the sidebar and the graph canvas lets the user resize the sidebar live; cosmos `fitView` re-runs on mouseup so the canvas adapts.

## 8. Cluster-target hover list

- [x] 8.1 In expanded mode, when hovering an other-anchor supernode, show the detail-nodes inside that cluster that connect into the expanded anchor (or to the currently-selected detail node, when one is active).
- [x] 8.2 Precede each entry with one colored swatch per distinct edge kind contributing to that connection; the list expands to fit all entries (no internal scrollbar).
- [x] 8.3 Build the data once per `buildDetailViewFiltered` call (`crossTargets` map) or once per selection (`selection.neighborsByAnchor`) so hover stays O(1).

## 9. CLI plumbing (kenn-cli)

- [x] 9.1 `cmd_analyze::run` derives `workspace_name` from the workspace path's final component (empty when none); passes it through `AnalyzeOptions`.
- [x] 9.2 Add `--graph [<spectral|force|stress|linlog>]` clap flag on `analyze`. Bare `--graph` falls back to `[analyze] graph_layout` from `kenn.toml`, then to `"spectral"`. Explicit values override the config; invalid values exit with an error before any output.
- [x] 9.3 Status line on stdout names both files when `--graph` was passed (`wrote <REPORT.md path> + <graph.html path> ...`), or REPORT.md alone otherwise.
- [x] 9.4 `cmd_init`'s starter `kenn.toml` lives as a separate `crates/kenn-cli/src/starter_kenn.toml` file loaded via `include_str!`. The starter ships an explicit `[tests] paths` block (no built-in fallback) with commented examples for project-specific patterns like `<Name>.Test/` sibling directories, and a commented `[analyze] graph_layout` line.

## 10. Config crate

- [x] 10.1 Create `crates/kenn-config/` housing the formerly-`kenn-indexer::config` module; workspace `members` list updated.
- [x] 10.2 Add `AnalyzeConfig { graph_layout: Option<String> }` and the `[analyze]` section to `Config`.
- [x] 10.3 Update `kenn-indexer`, `kenn-cli`, `kenn-mcp`, and `kenn-analyze` to depend on `kenn-config`; every `kenn_indexer::config::*` import rewritten to `kenn_config::*`. Indexer no longer re-exports a `config` module.
- [x] 10.4 Remove `TestsConfig::default_paths()` and `TestsConfig::effective()`. The `paths` field is authoritative — callers read `config.tests.paths` directly. Built-in fallback patterns live only in the starter `kenn.toml`.

## 11. Fallback removal

- [x] 11.1 `kenn_analyze::analyze` returns an `anyhow::bail!` error referencing `kenn index --force` when `projection::load_from_reader` returns an empty graph.
- [x] 11.2 Delete `projection::build`, `KEPT_RELATIONS`, `weight_for`, `is_module_like`; clean up `HashSet` and `SymbolRow` imports.
- [x] 11.3 Drop `AnalyzeReport.used_fallback` and the matching CLI warning.
- [x] 11.4 Rewrite `crates/kenn-analyze/tests/fallback.rs` as `analyze_errors_when_aggregate_tables_missing` — asserts the error references "aggregate".

## 12. Tests

- [x] 12.1 Layout unit tests (1.3) cover determinism + non-overlap.
- [x] 12.2 Aggregate-missing test in `crates/kenn-analyze/tests/fallback.rs` passes against the bail-out path.
- [x] 12.3 `cargo test --workspace` passes.

## 13. End-to-end validation

- [x] 13.1 Generate `graph.html` against a small TypeScript monorepo (~20 edges) — verify it renders in all-detail mode and the per-workspace title appears.
- [x] 13.2 Generate against a multi-crate Rust workspace (~3.9k edges) — verify it renders in all-detail with distinct anchor discs and cross-disc edges visible.
- [x] 13.3 Generate against a representative C# enterprise repo (~12k nodes / 121k edges) — verify it loads in overview mode with N supernodes + bundled edges; click a supernode expands; `Esc` returns to overview.
- [x] 13.4 In all three: open in a real browser, confirm the canvas is not blank, the simulation does not animate, point sizes do not change with zoom, edges are not clickable, and the selection-info pane appears on click.
- [x] 13.5 `cargo clippy --workspace --all-targets` clean on the new code; `cargo test --workspace` passes.
