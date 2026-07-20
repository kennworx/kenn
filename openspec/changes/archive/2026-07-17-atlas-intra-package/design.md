## Context

The atlas producer (`crates/kenn-indexer/src/atlas/producer.rs`, `build_concepts`)
groups every internal code symbol by its **anchor** (package/crate/module name,
resolved from the aggregate graph), emits one `package` concept per anchor, one
`document` concept per non-code top-level dir, and `domain` concepts for
**cross-anchor** flat-Louvain communities (`build_domains`). This is optimal for a
multi-package repo and near-useless for a monolithic library (one dominant
anchor). See the proposal's Alamofire evidence.

The producer already computes everything the fix needs: `sym_anchor` (symbol →
anchor), `path_of` (symbol → primary def file path), `central_nodes` /
`domain_eligible` (per-anchor ranked non-test nodes), `degree` (weighted
centrality), and the flat-community membership. The change is additive logic over
these, not a new data source — the atlas stays data-dependent (design invariant).

## Goals / Non-Goals

**Goals:** a monolithic/dominant package is mapped by its internal structure —
source-directory sub-areas + intra-package domains; bundled example/demo code
never fabricates a domain; multi-package repos are unchanged.

**Non-Goals:** changing indexer package granularity; sub-dividing *every* package
(only large dominant ones); cross-language module semantics beyond "top-level
source subdirectory."

## Decisions

### D1 — Subdivide only a large, dominant anchor

A package is subdivided into source sub-areas only when both hold:
- **Dominant** — the anchor owns a large share of the repo's code symbols (≥ a
  `DOMINANT_FRACTION`, e.g. 0.5) OR the repo has ≤ `FEW_ANCHORS` code anchors.
  (A repo of many balanced crates keeps flat packages — its cross-package domains
  already carry the structure.)
- **Structured** — its symbols span ≥ `MIN_SUBAREAS` (e.g. 2) distinct
  sub-directories, each with ≥ `MIN_SUBAREA_SYMBOLS` (e.g. 5) symbols. A flat
  package (all symbols in one dir) is left as a single concept.

Thresholds are fixed constants (determinism), tuned against Alamofire + kenn's
own atlas (which must NOT sprout sub-areas — its crates are already the right
grain).

### D2 — Sub-area = top-level source subdirectory under the package root

The **package root** is the longest common directory prefix of the anchor's
symbol def-file paths. Each symbol's sub-area is the **first path segment beneath
that root**, skipping a single conventional source wrapper (`Source`, `Sources`,
`src`, `lib`) so `Source/Core/Request.swift` → `Core`, not `Source`. Symbols
directly under the root (no subdir) fall into an implicit root sub-area that is
only emitted if it clears `MIN_SUBAREA_SYMBOLS`. Grouping is by exact segment
string; sorted for determinism.

### D3 — Sub-areas are a `component` concept parented to the package

A source sub-area is **code**, so it is neither a `document` (non-code doc dir,
this repo's renamed former "area") nor a top-level `package`. It gets a new
concept type **`component`** with a `parent` = its package concept id. Its body is
the same structural shape as a package (central symbols + members under
`<pkg>/<subarea>/`), ranked by the existing `degree` metric restricted to the
sub-area's symbols. The package concept keeps its own central list (the whole
package's top symbols) and gains a **Components** section linking its children.

*Open question O1: `component` vs hierarchical `package` (a package with a
parent). `component` keeps the top-level package axis clean and reads as
"in-package structure"; a parented package reuses one type but blurs "what is a
deployable unit." Leaning `component`.*

### D4 — Intra-package domains when the repo is single-dominant

`build_domains` today requires `cross_anchor` (span >1 anchor). Relax it: a
community qualifies as a domain when it clears `MIN_DOMAIN_SIZE` **and** either
(a) it spans >1 anchor (today's rule — always valid), OR (b) the repo is
single-dominant (D1's dominance test) and the community is a real semantic cluster
within the dominant anchor. Rule (b) only engages for single-dominant repos, so a
multi-package repo is never flooded with intra-package communities that just
shadow its packages. A domain and a `component` may overlap (one is
directory-structural, the other semantic-graph) — that is intended, two lenses.

### D5 — Exclude example / sample / demo code from domain + central eligibility

Mirror the existing test exclusion: a symbol whose def-file path contains a
conventional example/sample/demo/fixture segment (`example`, `examples`, `sample`,
`samples`, `demo`, `fixtures`, case-insensitive, as a full path segment) is
excluded from `domain_eligible` and from package/component central lists. This
kills the Alamofire `URLEncoding`-via-`iOS_Example` artifact at the source, and
generalizes (a bundled demo is never a repo's "central" concept). Example symbols
still count toward the package's member/symbol totals.

*Open question O2: is a hardcoded segment list right, or should it read a config
knob (like `[tests].paths`)? Leaning: a built-in list first (zero-config), a
config override deferred until a real repo needs it.*

### D7 — Package members: total + per-directory counts, not a capped file list

Today a package concept lists its top `MAX_MEMBERS` (6) files by symbol count — a
silent truncation that reads as "this package has 6 files" and, for a flat
non-decomposed package (the common case in a multi-package repo), hides the rest
with no component to recover them. This contradicts the component rule's own stated
principle ("one source directory → list ALL its files"): a flat package *is* one
source directory.

Replace the capped file list with a **structural summary** that is scale-invariant:
- The `## Files under <package>` heading states the **true total file count** — a cap
  can never again masquerade as the real count. (Section titled "Files under", not
  "Members": it counts files per directory, not individual members, and the old
  wording read as a truncated member list.)
- The body lists **every directory holding member files** (each file's exact parent
  directory, relative to the package root) with a per-directory file count, sorted
  count-desc then path-asc. Directory count ≪ file count, so this never becomes a
  wall of noise the way an uncapped file dump would.
- A **`component`** keeps its flat file list — it maps one directory, so the
  histogram would degenerate to a single line; the file names are the useful signal
  there.

Exact rendering — the heading names the package dir + carries the total (no
backticks), each line a directory + its count, directories relative to the package
root with no trailing slash:

```
## Files under Account.Data - 6

- src - 6
```
```
## Files under Foo - 47

- src/Core - 18
- src/Features/Auth - 5
- src/Features/Billing - 4
- src - 2
```

Implementation: the producer already builds a per-file `file_counts` map when
selecting members (`producer.rs:349`). Bucket those by parent directory into a
`dir_counts` list and carry a `file_count` total on `Concept`; drop the
`MAX_MEMBERS` cap for the package member path (components already list all files).
okf renders the package `## Members` from `file_count` + `dir_counts`, and the
component `## Members` from the flat member list as today.

*Open question O4: per-directory granularity — exact parent dir (`src/Features/Auth/`
and `src/Features/Billing/` as separate lines) vs rolled up to the top-level
sub-area (`src/Features/`). Leaning exact-parent (matches "count per dir" literally);
roll-up stays compact for deep trees. Pin during implementation against a real
multi-dir package.*

### D6 — Determinism

Every new grouping/threshold is a pure function of the persisted aggregate +
analysis tables and fixed constants — no wall-clock, sorted iteration throughout.
Re-indexing an unchanged repo yields byte-identical concept files (the atlas
determinism invariant), and the new sections are stable-ordered.

## Risks / Trade-offs

- **Threshold tuning is a heuristic.** Too eager → a mid-size package fragments;
  too shy → Alamofire stays flat. → Tune against two fixtures (Alamofire =
  should subdivide; kenn = should NOT), assert both in tests.
- **`component` is a new concept type.** OKF rendering + the reader/consumer must
  learn it. → Additive; unknown-type consumers ignore it. Scope kept to producer
  + okf.
- **Example-suppression false positives.** A production directory literally named
  `examples/` would be dropped from centrality. → Rare; accepted (it is still
  indexed and in member lists, only not "central").

## Migration Plan

No users; a plain reindex regenerates the atlas. Single-package repos get the
richer map; multi-package repos are byte-identical to today (the new paths don't
engage). No store schema change — the atlas is regenerated markdown.

## Open Questions

- **O1** — `component` concept type vs parented `package` (D3).
- **O2** — built-in example-segment list vs config knob (D5).
- **O3** — exact threshold values (`DOMINANT_FRACTION`, `FEW_ANCHORS`,
  `MIN_SUBAREAS`, `MIN_SUBAREA_SYMBOLS`, split-depth): pin during implementation
  against the two fixtures.
- **O4** — package member per-directory granularity: exact parent dir vs top-level
  sub-area roll-up (D7).
