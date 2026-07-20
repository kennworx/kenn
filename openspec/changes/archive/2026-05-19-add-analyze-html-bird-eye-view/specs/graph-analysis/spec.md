## ADDED Requirements

### Requirement: `graph.html` artifact

`kenn analyze` SHALL emit `kenn-out/graph.html` when the user opts in via the `--graph` flag (or via `[analyze] graph_layout` in `kenn.toml`, see below). The file SHALL be a single self-contained HTML document that opens directly in a modern browser (no server, no build step, no extraction). The only runtime dependency SHALL be a single ES-module script loaded from `https://esm.sh/@cosmos.gl/graph@2.6.4` (MIT-licensed). The HTML's `<title>` SHALL include the workspace's directory name so multiple `graph.html` tabs are distinguishable.

When `--graph` is not passed and no config default is set, `kenn analyze` SHALL write only `kenn-out/REPORT.md`.

#### Scenario: graph.html only emitted when requested

- **WHEN** `kenn analyze` runs without `--graph` and no `[analyze] graph_layout` is set in `kenn.toml`
- **THEN** `kenn-out/REPORT.md` MUST exist
- **AND** `kenn-out/graph.html` MUST NOT be created or modified by this run

#### Scenario: graph.html emitted when --graph passed

- **WHEN** `kenn analyze --graph` runs against any snapshot with aggregate tables
- **THEN** `kenn-out/REPORT.md` MUST exist
- **AND** `kenn-out/graph.html` MUST exist
- **AND** opening `graph.html` in a browser MUST render the aggregated graph without further user setup

#### Scenario: Per-workspace title

- **WHEN** `kenn analyze --graph` runs against workspace `/path/to/foo`
- **THEN** the emitted `graph.html` MUST contain a `<title>` element whose text includes `foo`

### Requirement: CLI flag and config default for graph emission

The `kenn analyze` command SHALL accept a `--graph [<algo>]` flag where the optional value selects the anchor layout algorithm. Bare `--graph` (no value) SHALL use the algorithm specified by `[analyze] graph_layout` in `kenn.toml` when set, falling back to `spectral` otherwise. Explicit `--graph spectral|force` SHALL override any config default.

Valid algorithm values are `spectral`, `force`, `stress`, and `linlog` (case-insensitive). Any other value SHALL cause the command to exit with an error before any output is written.

#### Scenario: Bare --graph uses config default

- **WHEN** `kenn.toml` contains `[analyze] graph_layout = "force"`
- **AND** the user runs `kenn analyze --graph`
- **THEN** the emitted `graph.html` MUST use the `force` algorithm

#### Scenario: Explicit value overrides config

- **WHEN** `kenn.toml` contains `[analyze] graph_layout = "force"`
- **AND** the user runs `kenn analyze --graph spectral`
- **THEN** the emitted `graph.html` MUST use the `spectral` algorithm

#### Scenario: Invalid algorithm rejected

- **WHEN** the user runs `kenn analyze --graph foo`
- **THEN** the command MUST exit with a non-zero code and an error message naming the valid values
- **AND** no `graph.html` MUST be written

### Requirement: Server-side deterministic layout

Node positions for the HTML view SHALL be computed in Rust (in `kenn_analyze::layout::compute`) and embedded in the JSON payload, not computed client-side. The layout SHALL be deterministic across runs: the same input graph and the same selected algorithm MUST produce byte-identical position arrays on every analyze. The browser SHALL paint the positions as given and SHALL NOT run any force simulation or iterative relayout.

The layout SHALL place each anchor as a disc (radius proportional to √node_count, clamped to a minimum) and place nodes inside each disc on a Fermat spiral so neither nodes nor discs concentrate at the center. Disc centers SHALL be computed by one of the supported algorithms:

- **`spectral`** (default): 2D embedding from the top two non-trivial eigenvectors of the random-walk smoothing matrix `D⁻¹W` of the anchor super-graph (deflated power iteration), followed by a short force-relaxation pass and a non-overlap cleanup pass.
- **`force`**: classical Fruchterman-Reingold force layout (per-anchor-normalized spring attraction along couplings, `k²/d` repulsion between all pairs, linear cooling, Hooke-like gravity toward origin, and a hard canvas-radius clamp), seeded from the spectral embedding.
- **`stress`**: stress majorization against graph-theoretic shortest-path distances (Dijkstra on the anchor super-graph with `1/√weight` edge lengths) — iteratively pulls every pair toward a Euclidean distance matching its graph-metric distance. Seeded from the spectral embedding.
- **`linlog`**: Noack's LinLog model — constant per-edge attraction (`F_a = norm(w)·base`, independent of distance) and logarithmic-energy repulsion (`F_r ∝ (rᵢ+rⱼ)/d`), so weakly-coupled pairs are still pulled together at any range. Seeded from the spectral embedding.

All algorithms SHALL produce a non-overlapping disc packing (a final overlap-resolution pass runs after the algorithm-specific refinement).

#### Scenario: Same snapshot + algorithm produces byte-identical positions

- **WHEN** `kenn analyze --graph spectral` runs twice against the same snapshot
- **THEN** the embedded JSON position arrays in the two `graph.html` files MUST be byte-identical

#### Scenario: Different algorithms may produce different positions

- **WHEN** `kenn analyze --graph spectral` and `kenn analyze --graph force` run against the same snapshot
- **THEN** the embedded positions MAY differ (each algorithm is independently deterministic)

#### Scenario: Browser does not animate

- **WHEN** `graph.html` finishes loading
- **THEN** no node MUST move from its painted position absent explicit user interaction (pan, zoom, mode switch)

### Requirement: Two display modes with edge-count threshold

The HTML SHALL pick one of two initial display modes based on the count of aggregate edges in the embedded data:

- When `edges.length ≤ 5000`: **all-detail** mode. Every aggregate node and aggregate edge is rendered.
- Otherwise: **overview** mode. One supernode per anchor (sized by √node_count) is rendered along with one bundled edge per unique anchor pair. The bundled edge's weight is the sum of weights of all aggregate edges crossing that pair.

In overview mode, clicking a supernode SHALL enter **expanded mode** for that anchor: the anchor's individual aggregate nodes appear at their precomputed positions, intra-anchor aggregate edges render between them, and cross-anchor aggregate edges from the expanded anchor render from their detail endpoint to the OTHER anchor's still-visible supernode. The other anchors SHALL remain represented as supernodes (and their inter-supernode bundled edges remain visible). Clicking another supernode SHALL swap the expanded anchor. The `Esc` key (or a "collapse" affordance) SHALL return to overview.

Clicking an anchor entry in the sidebar's anchor legend SHALL be equivalent to clicking that anchor's supernode (expand if not currently focused; collapse back to the default view if already focused).

#### Scenario: Small workspace renders all detail by default

- **WHEN** `graph.html` for a workspace with 1603 nodes / 3942 edges loads
- **THEN** the initial view MUST render all 1603 nodes and all 3942 edges
- **AND** no supernodes MUST be visible

#### Scenario: Large workspace renders overview by default

- **WHEN** `graph.html` for a workspace with 12162 nodes / 121608 edges / 366 anchors loads
- **THEN** the initial view MUST render 366 supernodes
- **AND** MUST render the bundled anchor-to-anchor edges
- **AND** MUST NOT render the underlying detail nodes / edges in this mode

#### Scenario: Click-to-expand in overview mode

- **WHEN** the user clicks a supernode in overview mode
- **THEN** the view MUST transition to expanded mode for that anchor
- **AND** the expanded anchor's detail nodes MUST appear at their precomputed positions
- **AND** the other anchors MUST remain as supernodes
- **AND** the status line MUST indicate `expanded <anchor>` with the anchor's node count

### Requirement: Selection stack with one-level undo

Clicking an unselected point SHALL select it and focus its 1-hop neighborhood (other elements rendered dimmed via the renderer's greyout opacity).

Clicking the **already-selected** point SHALL deselect it AND restore the previous selection if one existed. Selection state SHALL behave as a one-deep undo stack: select A → select B → click B again → A is re-selected → click A again → nothing is selected.

The `Esc` key SHALL clear the entire selection (both current and the one-deep stack). If a supernode is currently expanded, a subsequent `Esc` SHALL return to overview mode.

Mode/view changes (entering expanded mode, returning to overview, applying filters) SHALL clear any active selection.

Edges SHALL NOT be clickable. Only points SHALL receive click events.

#### Scenario: Click on selected node returns to previous selection

- **WHEN** the user clicks node A (now selected)
- **AND** the user clicks node B (now selected, A is the previous)
- **AND** the user clicks node B again
- **THEN** node A MUST be the active selection
- **AND** the 1-hop neighborhood focus MUST be A's neighborhood

#### Scenario: Esc cancels selection

- **WHEN** any selection is active
- **AND** the user presses `Esc`
- **THEN** the selection MUST be cleared
- **AND** no neighborhood focus MUST remain
- **AND** every visible element MUST be rendered at full opacity

#### Scenario: Clicks on edges are ignored

- **WHEN** the user clicks on a rendered edge
- **THEN** no selection change MUST occur
- **AND** any active selection MUST remain active

### Requirement: Selected-node info pane

When a point is selected, the HTML SHALL display a pinned info pane in the top-left of the canvas area (not the floating hover tooltip). The pane SHALL show the selected point's name, kind, language, anchor (with anchor color swatch), weighted degree, and any of `external` / `test` tags.

The pane SHALL clear when the selection is cleared (by any means: clicking the selected point twice, pressing `Esc`, switching modes, or clicking the stage).

The top-left area (selection pane + `mode-bar` "expanded: <anchor>" indicator) SHALL be laid out as a flex column so the two elements stack naturally without manual offset bookkeeping. The top-right hover panel uses the same flex container pattern.

#### Scenario: Selection produces a top-left info pane

- **WHEN** the user clicks a detail point in any mode
- **THEN** an info pane MUST appear in the top-left of the canvas area
- **AND** the pane MUST contain the point's name, kind, anchor, and weighted degree

#### Scenario: Pane clears on deselect

- **WHEN** a node is selected and its info pane is visible
- **AND** the user presses `Esc`
- **THEN** the info pane MUST be hidden

### Requirement: Cluster-target list on supernode hover

When the user hovers a supernode in expanded mode, the supernode's hover panel SHALL include a list of detail-nodes inside that cluster that have edges crossing into the expanded anchor:

- If a single detail node is currently selected, the list SHALL contain only nodes that connect to that specific selection.
- Otherwise the list SHALL contain every node in the hovered cluster that connects to ANY node in the expanded anchor.

Each entry SHALL display the target node's name preceded by one colored swatch per distinct edge kind contributing to that connection. The colors SHALL match the kind palette used by the rest of the UI. The list SHALL expand to fit all entries (no internal scrollbar).

#### Scenario: Hover shows selection-specific connections

- **WHEN** the user is in expanded mode and has selected detail node `X`
- **AND** the user hovers a supernode of cluster `C`
- **THEN** the hover panel MUST show the subset of `C`'s nodes that have a graph edge to `X`
- **AND** each entry MUST display a colored swatch for each distinct edge kind that contributes to the connection

#### Scenario: Hover without selection shows whole-cluster connections

- **WHEN** the user is in expanded mode with no detail node selected
- **AND** the user hovers a supernode of cluster `C`
- **THEN** the hover panel MUST show every node in `C` that has at least one edge to a node in the expanded anchor

### Requirement: WebGL renderer with constant pixel sizing

The HTML SHALL use `@cosmos.gl/graph` (MIT) as the WebGL renderer. Point sizes SHALL be in screen pixels and SHALL NOT scale with zoom (`scalePointsOnZoom: false`). Link widths SHALL also be in screen pixels (`scaleLinksOnZoom: false`). The simulation SHALL be disabled via `disableSimulation: true` and additionally paused immediately after the first render as a defense in depth.

The CC-BY-NC-4.0 `@cosmograph/cosmograph` higher-level UI bundle SHALL NOT be loaded — only the MIT lower-level `@cosmos.gl/graph` engine.

The current selection state SHALL be cleared from the renderer (via `unselectPoints()` or equivalent) at every view switch — otherwise cosmos retains the previous greyout mask across `setPointPositions`/`setLinks` calls and the new view paints with stale dimming applied.

#### Scenario: Zoom does not change point or edge thickness

- **WHEN** the user zooms in or out
- **THEN** every visible point's on-screen pixel size MUST remain the same
- **AND** every visible edge's on-screen pixel thickness MUST remain the same

#### Scenario: Non-commercial bundle is not loaded

- **WHEN** the generated `graph.html` is inspected
- **THEN** the script tag MUST reference `@cosmos.gl/graph` (MIT)
- **AND** MUST NOT reference `@cosmograph/cosmograph` (CC-BY-NC-4.0)

### Requirement: Live filter and search wiring

The sidebar SHALL provide live, debounced filter and search controls that update the rendered view without a page reload:

- **Per-kind edge toggles**: one checkbox per edge kind present in the data, with a colored swatch matching the kind palette. Unchecking a kind removes those edges from the current view.
- **`hide external packages`**: removes nodes whose `external` flag is true. In overview mode, anchors whose every node is excluded by this filter SHALL also be removed (their supernode and bundled edges disappear).
- **`hide test code`**: same behaviour as above for `test`.
- **Search input** (debounced ~120 ms): in overview mode matches against anchor name; in detail mode matches against node name.

Any filter change SHALL rebuild the current view from scratch. Edges whose endpoint(s) are filtered out in all-detail mode SHALL be dropped (never re-routed to point index 0).

#### Scenario: Hiding external removes external-only anchors in overview

- **WHEN** the user checks `hide external packages` in overview mode
- **AND** an anchor's nodes are entirely external (e.g. `System.Text.Json`)
- **THEN** that anchor's supernode MUST disappear
- **AND** every bundled edge touching it MUST disappear

#### Scenario: Filtered detail nodes don't leave stray edges

- **WHEN** the user enables `hide external packages` in all-detail mode
- **AND** an edge has one endpoint that is now filtered out
- **THEN** the edge MUST NOT be rendered (and MUST NOT collapse to the canvas origin)

### Requirement: Aggregate snapshot required

`kenn_analyze::analyze` SHALL return an error if the snapshot does not contain the aggregate-graph tables (i.e. `projection::load_from_reader` returns an empty graph). The error message SHALL instruct the user to rebuild the index with `kenn index --force`. No fallback in-memory recomputation of the projection SHALL occur.

#### Scenario: Pre-Phase-2 snapshot is rejected

- **WHEN** `kenn analyze` runs against a snapshot lacking aggregate tables
- **THEN** the command MUST exit with an error
- **AND** the error message MUST reference `kenn index --force`

### Requirement: Streaming HTML write

`kenn-analyze` SHALL stream the HTML directly to a buffered file writer (`serde_json::to_writer` for the payload) rather than building the full HTML string in memory. Peak Rust memory used by the HTML emission SHALL stay bounded by serde's internal row buffers, not by the size of the output file.

The HTML template SHALL be stored in a separate `graph.html` file alongside `graph.rs` (the renderer module) and included at compile time via `include_str!`, so the template can be edited with proper HTML/JS syntax highlighting and the Rust source stays focused on payload assembly. The starter `kenn.toml` written by `kenn init` SHALL similarly live as a separate `starter_kenn.toml` file next to `cmd_init.rs`.

#### Scenario: Large output does not allocate the full file in memory

- **WHEN** `kenn analyze --graph` runs against a workspace whose `graph.html` is ~8 MB
- **THEN** the analyze process MUST NOT allocate a Rust `String` or `Vec<u8>` whose length approaches the file size
- **AND** the file MUST be written via a buffered `Write` implementation

### Requirement: Cross-crate configuration

The `kenn.toml` configuration MUST live in a dedicated `kenn-config` crate at `crates/kenn-config/`, not inside `kenn-indexer`. Every crate that loads or consumes `Config` (currently `kenn-indexer`, `kenn-cli`, `kenn-mcp`, `kenn-analyze`) SHALL depend on `kenn-config` rather than on `kenn-indexer::config`.

The `Config` struct SHALL include an optional `[analyze]` section with at least one field: `graph_layout: Option<String>` controlling the bare-`--graph` default algorithm.

The `[tests] paths` glob list SHALL be authoritative — no hard-coded fallback patterns live in code. An empty list means "no files are test code". The starter `kenn.toml` written by `kenn init` SHALL ship an explicit set of conventional patterns so a fresh workspace gets sensible behaviour out of the box, with project-specific extensions (e.g. .NET `<Name>.Test/` sibling directories) shown as commented examples.

#### Scenario: Crates use the shared config crate

- **WHEN** a workspace crate needs to load `kenn.toml`
- **THEN** it MUST import `Config` from `kenn_config`
- **AND** it MUST NOT depend on `kenn-indexer` solely to access the config type

#### Scenario: No hard-coded test glob fallback

- **WHEN** a user clears `[tests] paths` to an empty list
- **THEN** no files MUST be flagged as test code by the indexer
- **AND** the code MUST NOT substitute a built-in default list
