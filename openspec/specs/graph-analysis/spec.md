# graph-analysis

## Purpose

Defines the aggregated graph artifact persisted alongside a kenn snapshot and the `kenn analyze` command that consumes it. The aggregated graph collapses per-symbol nodes into their nearest class-like or module-like enclosing symbol, producing a weighted undirected projection suitable for community detection. `kenn analyze` clusters this projection both anchored (per package / path-prefix) and flat (across the whole workspace) and renders the result as `kenn-out/REPORT.md`.
## Requirements
### Requirement: Aggregated graph as snapshot artifact

The kenn snapshot SHALL include an aggregated graph computed from the per-symbol graph. The aggregated graph collapses methods, fields, free functions, parameters, constants, and other non-grouping symbols into their nearest enclosing class-like (`class`, `struct`, `trait`, `interface`, `enum`, `type_alias`) or, failing that, module-like (`module`, `namespace`, `package`) symbol. For each kept edge kind, every unique pair of aggregate endpoints becomes ONE weighted undirected edge of that kind. Multiple kinds between the same aggregate pair produce separate edges (one per kind); the total weight of an edge is the sum of the per-kind weights of all per-symbol edges of that kind that fall on that aggregate pair.

The kept edge kinds and their weights SHALL be:

| Kind | Weight |
|---|---|
| `calls` | 3 |
| `type_use` | 2 |
| `field_access` | 2 |
| `implements` | 2 |
| `instantiates` | 2 |
| `overrides` | 1 |
| `imports` (module → module only) | 1 |

`defined_in`, `contains`, `generic_constraint`, and `corresponds_to` SHALL be skipped. Self-loops on the aggregated graph SHALL be dropped.

#### Scenario: Method calls aggregate to class-level edge

- **WHEN** a method `A.foo()` calls a method `B.bar()` in the per-symbol graph
- **THEN** the aggregated graph MUST contain one undirected edge between aggregate nodes `A` and `B` with `calls` kind
- **AND** the edge weight MUST be 3

#### Scenario: Multiple calls between same aggregates accumulate

- **WHEN** methods of class `A` call methods of class `B` from N distinct per-symbol edge pairs
- **THEN** the aggregated `calls` edge between `A` and `B` MUST have weight `3 * N`

#### Scenario: Free functions in same module produce self-loop and are dropped

- **WHEN** free function `mod::a()` calls free function `mod::b()` and both roll up to the same module aggregate
- **THEN** the aggregated graph MUST NOT contain that edge

#### Scenario: Multiple kinds between same aggregates produce separate edges

- **WHEN** methods of class `A` both call and type-use members of class `B`
- **THEN** the aggregated graph MUST contain two distinct undirected edges between `A` and `B`: one with kind `calls` and one with kind `type_use`
- **AND** each edge's weight MUST be the sum of the per-kind weight times the count of per-symbol edges of that kind

### Requirement: Anchor assigned to every aggregate node

Every aggregate node SHALL be assigned an anchor identifier and human-readable name, determined by:

1. The symbol's `pkg` short id when non-zero.
2. The first path segment (workspace-relative, forward-slash-separated) of the symbol's primary def file when `pkg` is zero.
3. The literal `"<unanchored>"` when neither is available.

Anchors form the top level (L0) of the hierarchical clustering view. Anchor names are persisted alongside the aggregate node record so renderers do not have to re-resolve them.

#### Scenario: C# symbol uses package anchor

- **WHEN** a C# symbol from package `Foo.Bar` (pkg short id non-zero) is aggregated
- **THEN** its aggregate node MUST record the anchor as the package's name

#### Scenario: Rust symbol uses path-prefix fallback

- **WHEN** a Rust symbol with `pkg = 0` is aggregated, and its primary def file is `crates/kenn-indexer/src/transform/document/walk.rs`
- **THEN** its aggregate node MUST record the anchor as `crates/kenn-indexer`

#### Scenario: Symbol with no def file uses the unanchored bucket

- **WHEN** a symbol has neither a non-zero `pkg` nor a def file
- **THEN** its aggregate node MUST record the anchor as `<unanchored>`

### Requirement: Anchored hierarchical clustering

`kenn analyze` SHALL produce an anchored hierarchical clustering of the aggregated graph. L0 partitions nodes by anchor. Within each L0 partition, single-level Louvain runs on the induced subgraph to produce L1 communities. Each L1 community with at least `min_cluster` nodes (default 20) recurses with Louvain on its own induced subgraph to produce L2, and so on up to `max_depth` (default 4). Communities below `min_cluster` SHALL be leaf nodes.

Both `min_cluster` and `max_depth` SHALL be configurable through CLI flags (`--min-cluster N`, `--max-depth N`).

Hierarchical clustering SHALL be deterministic: identical input graphs MUST produce identical hierarchies including stable level ids. Stability is achieved by sorting all iteration orders by `ShortId` (or anchor name, then community size desc, then min member id asc, for ids).

#### Scenario: Same aggregated graph clusters identically across runs

- **WHEN** `kenn analyze` is run twice against the same snapshot with the same parameters
- **THEN** both runs MUST produce identical hierarchical structures, including the same community assignments and the same level ids

#### Scenario: max_depth bounds recursion

- **WHEN** `kenn analyze --max-depth 2` is run on a graph whose anchor contains a deeply modular subgraph
- **THEN** the hierarchy MUST NOT contain communities at depth greater than 2

#### Scenario: min_cluster halts recursion early

- **WHEN** a community at depth 2 has 15 nodes and `--min-cluster 20`
- **THEN** that community MUST be a leaf (no L3 children)

### Requirement: Flat clustering as cross-check

`kenn analyze` SHALL additionally run single-level Louvain over the entire aggregated graph (ignoring anchors) and render the resulting flat communities alongside the anchored hierarchy. For each flat community, the report MUST list the set of distinct anchors its members belong to and flag communities that span more than one anchor.

#### Scenario: Flat community contained within one anchor

- **WHEN** every member of a flat community has the same anchor
- **THEN** the report MUST list that community's anchor without a cross-anchor flag

#### Scenario: Flat community spans multiple anchors

- **WHEN** a flat community has members from anchors `A`, `B`, and `C`
- **THEN** the report MUST list all three anchors (up to a configurable limit) and MUST flag the community as cross-anchor

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

### Requirement: Analysis is a persisted snapshot artifact

The derived analysis (anchor map, hierarchical Louvain partition, flat Louvain partition, god-node rankings) SHALL be computed during `kenn index` and persisted as tables in the snapshot DB. Subsequent reads (the `kenn visualize` command and the MCP read tools) SHALL load this data via `Reader::scan_analysis_*` rather than recomputing.

The persisted set SHALL include:

- Top-N nodes by weighted degree for each of the three node filters (`live`, `test`, `external`).
- One row per flat community summarising size, total weight, anchor coverage, and the cross-anchor flag.
- One row per anchored-hierarchy community covering the recursive Louvain partition (depth ≥ 0, parent pointer when depth > 0), size, test-ratio, and the test-infra flag.
- One row per aggregate node mapping `short_id → (flat_community_id, anchored_leaf_community_id)` so per-symbol lookups are O(1).

#### Scenario: Analysis written at index time

- **WHEN** `kenn index` runs with `[index] persist_analysis = true` (the default)
- **THEN** the resulting snapshot MUST contain non-empty `analysis_god_nodes`, `analysis_flat_communities`, `analysis_anchored_hierarchy`, and `analysis_node_membership` tables

#### Scenario: Re-read instead of recompute

- **WHEN** `kenn visualize` runs against a snapshot whose analysis tables are populated
- **THEN** the command MUST NOT call `cluster::hierarchical`, `cluster::louvain_flat`, or `top_by_weighted_degree`
- **AND** it MUST load the persisted analysis via `Reader::scan_analysis_*`

### Requirement: REPORT.md written at index time

`kenn-out/REPORT.md` SHALL be rendered and written by the `kenn index` pipeline (in the analysis phase, after aggregation), not by the visualize / former-analyze command. The report's content SHALL be unchanged from the prior format (summary, three god-node sections, anchored hierarchy, flat communities). Emission SHALL be gated by `[index] write_report` (default `true`).

When `[index] write_report = false`, REPORT.md SHALL NOT be created or modified by the index run; any existing REPORT.md from a prior run SHALL be left untouched.

`kenn visualize` SHALL NOT write REPORT.md under any flag combination.

#### Scenario: Report emitted at index by default

- **WHEN** `kenn index` runs against a fresh workspace
- **THEN** `kenn-out/REPORT.md` MUST exist with the documented sections
- **AND** the file mtime MUST match the indexing run, not a later `kenn visualize` invocation

#### Scenario: Report suppression honored

- **WHEN** `kenn.toml` contains `[index] write_report = false`
- **AND** `kenn index` runs
- **THEN** `kenn-out/REPORT.md` MUST NOT be created (or overwritten) by this run

### Requirement: `kenn visualize` command surface

The CLI subcommand SHALL be named `kenn visualize`. It SHALL read the snapshot (including the persisted analysis), compute the anchor layout, and write `kenn-out/graph.html`. It SHALL accept:

- `--algo <spectral|force|stress|linlog>` — anchor layout algorithm. Bare `kenn visualize` resolves the algorithm from `[visualize] layout` in `kenn.toml`, falling back to `spectral` when unset. Explicit `--algo` overrides the config.
- `--workspace <path>` — same semantics as on other subcommands.

The command SHALL exit with an error and a non-zero code when the snapshot lacks the analysis tables, with a message referencing `kenn index --force`. The error message format SHALL match the message used by the existing missing-aggregate guard.

The command SHALL NOT recompute clustering or god-nodes. The command SHALL NOT write REPORT.md.

#### Scenario: Visualize reads persisted analysis and writes graph.html only

- **WHEN** `kenn visualize` runs against a snapshot whose analysis tables are populated
- **THEN** `kenn-out/graph.html` MUST be (re)written
- **AND** `kenn-out/REPORT.md` MUST NOT be modified

#### Scenario: Visualize errors on snapshots without analysis

- **WHEN** `kenn visualize` runs against a snapshot whose `analysis_god_nodes` table is empty or absent
- **THEN** the command MUST exit with a non-zero code
- **AND** stderr MUST reference `kenn index --force`

### Requirement: Analysis options live in `[index]` and `[visualize]`

The `kenn.toml` schema SHALL expose two sections relevant to analysis:

- `[index]` controls the index-time analysis + report writers:
  - `write_report: bool` (default `true`).
  - `persist_analysis: bool` (default `true`). When `false`, the analysis tables are not written and REPORT.md is not written regardless of `write_report`.
- `[visualize]` controls the visualize command:
  - `layout: Option<String>` (default unset). Sets the default `--algo` value used by bare `kenn visualize` invocations.

The previous `[analyze]` section SHALL no longer be recognised.

#### Scenario: Bare visualize uses config algorithm

- **WHEN** `kenn.toml` contains `[visualize] layout = "force"`
- **AND** the user runs `kenn visualize`
- **THEN** the emitted `graph.html` MUST use the `force` algorithm

### Requirement: clustering records graph-structure counters into stats

The analysis pass (`kenn-analyze`) SHALL record graph-structure counters into
the `stats` table with `subset='graph'` when it builds the community/centrality
clusters for a snapshot. The counters SHALL be derived from the same
`AnalysisResult` / `AnalysisRecords` the pass already produces (no extra graph
traversal), and written via the `write_stats` writer operation alongside the
`analysis_*` tables.

Counters that attribute to a language (their nodes carry one) SHALL be recorded
**per language** (`scope='language'`, `key=<language>`):
- `nodes` — aggregate nodes of that language;
- `god_nodes` — high-centrality hub nodes of that language;
- `anchors` — anchors of that language;
- `communities` — flat communities whose plurality member language is that
  language (an anchor/community spans languages and has none itself).

Counters that describe the whole partition SHALL be recorded once
(`scope='global'`, `key=''`):
- `hierarchy_depth` — maximum depth of the anchored hierarchy;
- `cross_anchor_communities` — communities spanning more than one anchor. This is
  the RAW clustering diagnostic: every such community, before any selection. It
  SHALL keep this meaning. The earned domain count is a SEPARATE counter written
  by the aggregation stage (see "the aggregation stage records the earned domain
  count"), because the earned-span rule lives in `kenn-indexer` and this pass may
  not depend on it.

Raw per-language edge counts SHALL NOT be recorded — an edge spans two nodes
that may be two languages and is not always symbol-sourced, so the count is
neither meaningful per language nor reconcilable with a whole-table total.

The analysis pass is optional in the pipeline; when it does not run, the
`subset='graph'` rows are absent and consumers treat them as unavailable (the
entity counts from `finalize` are unaffected).

#### Scenario: Per-language graph counters written during analysis

- **GIVEN** indexing runs with the analysis (clustering) pass enabled
- **WHEN** the pass writes the `analysis_*` tables
- **THEN** `stats` contains `(scope='language', key=<lang>, subset='graph', metric='god_nodes'|'communities'|'nodes'|'anchors')`
  rows per language
- **AND** `(scope='global', key='', subset='graph', metric='hierarchy_depth')` and
  `cross_anchor_communities` rows
- **AND** no raw per-language `edges` rows are written

#### Scenario: Analysis skipped leaves entity counts intact

- **GIVEN** indexing runs without the analysis pass
- **WHEN** the snapshot is published
- **THEN** `stats` has the `finalize` entity-count rows (language/manager)
- **AND** has no `subset='graph'` rows

### Requirement: the aggregation stage records the earned domain count

The aggregation stage (`kenn-indexer`) SHALL record the EARNED cross-package
domain count as a `(scope='global', key='', subset='graph', metric='domains')`
stat row: the communities that clear the domain axis's floors, where a package
joins a community's span only with enough members AND a first-party edge to
another qualifying package. This is the number the atlas renders and a domains
query returns.

It SHALL be computed by the SAME implementation of the earned-span rule that the
atlas producer and the domains query use, so a third surface cannot report a
different answer for one snapshot.

The aggregation stage SHALL compute it by reading the persisted community tables
(`analysis_flat_communities`, `analysis_node_membership`) back on its own writer
connection — never by recomputing clustering, and never by depending on
`kenn-analyze`, which the atlas capability already forbids. Writing it here
rather than in the analysis pass is what keeps that constraint intact while still
producing the row on every path.

The row SHALL NOT be conditional on the atlas bundle being built — a counter
present only on runs that rendered the atlas is a worse contract than the
inconsistency it replaces. It SHALL be written only when clustering produced
communities, so an absent row means the analysis pass did not run, which is
exactly when `cross_anchor_communities` is also absent.

`domains` and `cross_anchor_communities` are distinct questions over different
candidate sets, and NEITHER bounds the other. No ordering invariant between them
may be asserted: a multi-package repo typically reports far fewer earned than
raw, while a single-package repo reports `cross_anchor_communities = 0` with a
non-zero `domains`, because the axis deliberately keeps within-anchor clusters
for a repo that one package dominates.

#### Scenario: The earned count matches what the axis reports

- **GIVEN** a snapshot whose analysis pass ran
- **WHEN** the `domains` stat row is compared with what a domains query returns
  for that same snapshot, and with the count the atlas index header states
- **THEN** all three agree
- **AND** they agree because they share one implementation of the rule, not
  because separate copies happen to match

#### Scenario: The earned count is written without an atlas

- **GIVEN** indexing runs with clustering enabled but the atlas bundle disabled
- **WHEN** the snapshot is published
- **THEN** the `domains` stat row is still present

#### Scenario: A single-package repo inverts the two counters

- **GIVEN** a repo in which one package holds the majority of eligible nodes
- **WHEN** the counters are recorded
- **THEN** `cross_anchor_communities` MAY be `0` while `domains` is non-zero
- **AND** neither is treated as an error, because the domain axis keeps
  within-anchor clusters for such a repo

