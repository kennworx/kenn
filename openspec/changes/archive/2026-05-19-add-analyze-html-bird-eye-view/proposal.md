## Why

`kenn analyze` already produces a markdown REPORT.md that's useful for skimming but does not show structure visually. A reader wants to see which packages cluster together, which packages bridge to which, and where the heavyweight relations are — at a glance, at any repo size. Without a visual artifact, that signal stays locked in the JSON.

## What Changes

- `kenn analyze` gains a `--graph [<algo>]` flag that emits `kenn-out/graph.html` — a single self-contained file that renders the snapshot's aggregated graph as a static bird's-eye view in the browser. No server, no build step, no animation. Without the flag, only `REPORT.md` is written, same as before. `kenn.toml` gains an `[analyze] graph_layout = "spectral|force"` field that sets the default algorithm used by bare `--graph`; explicit `--graph spectral|force` always wins.
- **Layout is precomputed in Rust** with a choice of two deterministic algorithms:
  - **`spectral`** (default): 2D embedding from the top two non-trivial eigenvectors of the random-walk smoothing matrix of the anchor super-graph (deflated power iteration), followed by a short force-relaxation refinement, then a non-overlap cleanup pass. Strongly-coupled anchors land near each other globally.
  - **`force`**: classical Fruchterman-Reingold seeded from a sunflower spiral with per-anchor-normalized spring attraction, `k²/d` pairwise repulsion, and linear cooling. Useful when spectral places a weakly-connected pair on opposite sides.
  Within each anchor's disc, nodes are placed on a Fermat spiral (`r ∝ √(i/n)`, `θ = i × goldenAngle`) with a small inner blank ring so heavy nodes don't all converge to a single hub point. Both algorithms produce byte-deterministic positions for the same snapshot + algorithm.
- **Two display modes** chosen by edge count:
  - **All-detail** (≤5k edges): every per-symbol aggregate node and edge is shown.
  - **Overview → expand** (>5k edges): one supernode per anchor + bundled anchor-to-anchor edges by default. Click a supernode (or click an anchor in the sidebar legend) → that anchor expands into its disc of detail nodes + intra-anchor edges + cross-anchor connections to other supernodes. Click another supernode to swap. Click the focused anchor in the sidebar again, or press `Esc`, to collapse back to the default view.
- **WebGL renderer (`@cosmos.gl/graph` v2.6.4, MIT)** loaded from CDN, with simulation disabled, point sizes in screen pixels (zoom-invariant), and screen-pixel link widths. Scales to the 12k-node/121k-edge enterprise repo.
- **Sidebar**: counts, debounced search by node/anchor name, per-kind edge toggles, `hide external packages` and `hide test code` filters (live — anchors whose every node is filtered out are also removed from overview, and no orphan edges point to point 0 when endpoints are filtered), anchor legend with colors and node counts (clickable to expand into that anchor / collapse).
- **Selection UX**:
  - Clicking a point selects it and focuses its 1-hop neighborhood (everything else dims).
  - Clicking the **already-selected** point deselects it and restores the **previous selection** if one existed (selection acts as a one-deep stack — drill in, click to step back).
  - The selected point's info pane (name, kind, language, anchor, weighted degree, tags) renders in a top-left flex column under the mode bar — pinned, not the floating hover tooltip.
  - `Esc` cancels the current selection entirely. If no selection is active but a supernode is expanded, `Esc` returns to overview.
  - Mode/view changes (including filter rebuilds) clear the selection AND call `graph.unselectPoints()` so cosmos doesn't paint the new view with the previous greyout mask.
- **Cluster-target list on supernode hover** (expanded mode): hovering another anchor's supernode lists the detail nodes inside that cluster that connect into the expanded anchor — narrowed to the currently-selected node when a selection is active, otherwise showing every cross-anchor target. Each entry is preceded by colored edge-kind swatches.
- `AnalyzeOptions` gains `workspace_name` (per-workspace HTML `<title>`) and `graph_layout: Option<LayoutAlgo>`; `AnalyzeReport.html_path` is `Option<PathBuf>`. `cmd_analyze::run` derives `workspace_name` from the workspace directory, parses the `--graph` flag with the kenn.toml default as fallback, and prints both file paths when graph was emitted.
- `html::render` streams directly to a writer (`serde_json::to_writer`) — no intermediate `String`, so memory is bounded even for the ~8 MB enterprise HTML. The HTML template lives in a separate `graph_template.html` file alongside `html.rs` and is loaded with `include_str!` so it can be edited with proper syntax highlighting.
- The pre-Phase-2 fallback path was removed: `kenn analyze` now errors out with `snapshot has no aggregate-graph tables — run \`kenn index --force\` to rebuild` instead of recomputing the projection in memory.
- The `Config` type was extracted from `kenn-indexer` into a new `kenn-config` crate, shared by every crate that loads `kenn.toml`. This is the home of the new `[analyze]` section.

## Capabilities

### New Capabilities
- _(none)_

### Modified Capabilities
- `graph-analysis`: extends the `kenn analyze` artifact set with an opt-in `graph.html` alongside `REPORT.md`. Adds requirements covering deterministic Rust layout (`spectral` or `force`), the static (no-simulation) rendering contract, the two display modes and their selection rule, the click-to-expand interaction (supernode click or sidebar legend), the per-workspace title, the live filter / cluster-target hover list, the `--graph` CLI flag + `[analyze] graph_layout` config default, the dependency surface (single CDN script load, MIT-licensed renderer, no build step), and the hard-error policy when aggregate tables are absent. Tightens config layering by requiring the shared `kenn-config` crate.

## Impact

- **Code**:
  - New crate `kenn-config` at `crates/kenn-config/` housing the formerly-`kenn-indexer::config` module. Every config consumer (`kenn-indexer`, `kenn-cli`, `kenn-mcp`, `kenn-analyze`) depends on it.
  - New module `crates/kenn-analyze/src/layout.rs` (~330 LOC) — `LayoutAlgo` enum, spectral seed via deflated power iteration, force layout, sunflower fallback, anchor coupling builder, non-overlap cleanup, per-anchor Fermat spiral node placement.
  - Rewritten `crates/kenn-analyze/src/html.rs` (~240 LOC of Rust, plus 750-line `graph_template.html`) — streams to writer, embeds JSON payload (nodes, edges, supernodes, bundled anchor edges, kinds, anchors), CDN-loads `@cosmos.gl/graph` via esm.sh, implements the selection stack / info pane / filter wiring / cluster-target hover entirely client-side.
  - `crates/kenn-analyze/src/lib.rs`: `AnalyzeOptions { workspace_name, graph_layout }`, `AnalyzeReport { html_path: Option<PathBuf>, ... }` (no more `used_fallback`), `analyze()` errors on empty aggregate tables and conditionally emits `graph.html`.
  - `crates/kenn-cli/src/main.rs` + `cmd_analyze.rs`: `--graph [<algo>]` clap flag, parses against `LayoutAlgo`, falls back to `[analyze] graph_layout` from `kenn.toml`.
  - `crates/kenn-cli/src/cmd_init.rs`: starter `kenn.toml` gains a commented `[analyze]` block.
  - `crates/kenn-analyze/src/projection.rs`: `projection::build` and its helpers (`KEPT_RELATIONS`, `weight_for`, `is_module_like`, `HashSet`/`SymbolRow` imports) deleted with the fallback path.
- **Dependencies** (Rust): `serde` + `serde_json` added to `kenn-analyze`; `kenn-config` brings in `serde`, `toml`, `thiserror`.
- **Dependencies** (browser, runtime CDN, not vendored): `@cosmos.gl/graph@2.6.4` (MIT) via `https://esm.sh/`.
- **License posture**: the higher-level `@cosmograph/cosmograph` is CC-BY-NC-4.0 and is explicitly NOT used; only the MIT-licensed lower-level `@cosmos.gl/graph` engine is loaded.
- **Performance budget**: spectral seed is O(`ITERS × (N + E_anchor)`) per dimension — sub-millisecond on 366 anchors; the optional force layout is O(`ITERS × N²`) per iteration, ~3 s in debug on the 12k/121k workspace, sub-second in release. HTML write is streaming; peak Rust memory is bounded by serde's row buffers. Browser-side rendering: WebGL handles 121k edges; initial paint < 1 s.
- **Backwards compatibility**: REPORT.md output unchanged. CLI surface adds `--graph [<algo>]`; bare `kenn analyze` is byte-equivalent to before. Older `AnalyzeReport` consumers that read `html_path` as a non-optional path need to handle `Option`. Snapshots without aggregate tables now error out (used to be a fallback recompute + warning).
- **MCP**: unchanged.
