## ADDED Requirements

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

- **WHEN** a Rust symbol with `pkg = 0` is aggregated, and its primary def file is `crates/kenn-indexer/src/transform.rs`
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

### Requirement: REPORT.md structure

The analyze output `kenn-out/REPORT.md` SHALL contain, in order:

1. A summary section with total node count broken down by live / test / external, total undirected edge count, total weight, and community count.
2. Three god-node sections: User (Live), User (Tests), User (System / External), each listing the top-N nodes by weighted degree (default `top_n = 20`).
3. The anchored hierarchy: one section per anchor showing the hierarchical structure with per-community size, test ratio, and a "— test infra" tag for communities whose test ratio is at least 60%.
4. A flat-cross-check section listing flat communities with their anchor membership and cross-anchor flags.

#### Scenario: REPORT.md contains all required sections

- **WHEN** `kenn analyze` runs successfully against any snapshot
- **THEN** `kenn-out/REPORT.md` MUST contain sections titled "Summary", "God Nodes — User (Live)", "God Nodes — User (Tests)", "God Nodes — System / External", "Anchored Hierarchy", and "Flat Communities (cross-check)"

#### Scenario: Test infra tag applied at threshold

- **WHEN** a community contains 14 of 20 members marked `test = true` (70%)
- **THEN** that community's header MUST include "— test infra"

### Requirement: Fallback on pre-aggregate snapshots

When `kenn analyze` runs against a snapshot that does not contain the aggregated-graph tables (a snapshot indexed before this change), it SHALL recompute the projection in memory from `scan_symbols` and `scan_edges`, run clustering, and render the report normally. The CLI MUST print a single-line warning explaining the snapshot pre-dates the aggregated artifact and recommending `kenn index --force`.

#### Scenario: Pre-aggregate snapshot analyzed successfully with warning

- **WHEN** `kenn analyze` runs against a snapshot whose `aggregate_nodes` table is empty or absent
- **THEN** the report MUST be produced normally
- **AND** stderr MUST contain exactly one warning line referencing the snapshot age and suggesting `kenn index --force`

### Requirement: CLI surface

The `kenn analyze` subcommand SHALL accept the following flags:

- `--top-n N` — number of entries shown in each god-node list (default 20).
- `--max-depth N` — maximum hierarchy depth, anchor counted as depth 0 (default 4).
- `--min-cluster N` — minimum community size to recurse into (default 20).

Invocations without flags SHALL produce a report with the documented defaults.

#### Scenario: Bare invocation uses defaults

- **WHEN** `kenn analyze` runs with no flags
- **THEN** `kenn-out/REPORT.md` MUST be produced with `top_n = 20`, `max_depth = 4`, `min_cluster = 20`
