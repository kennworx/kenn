# atlas-bundle Specification

## Purpose
TBD - created by archiving change atlas. Update Purpose after archive.
## Requirements
### Requirement: kenn index emits an OKF atlas bundle

`kenn index` SHALL write an [OKF v0.1](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md)-conformant
markdown bundle derived from the code graph. The bundle SHALL be written at a
`Layout`-resolved, kenn-owned location — never a path hardcoded by a consumer —
so it resolves correctly for the local repo, a foreign workspace
(`kenn index -d <path>`), a worktree, or a custom store. The bundle SHALL contain
one concept document per **internal (non-external) package** (external dependency
packages SHALL be excluded), plus `document` concepts for first-party non-code
directories (e.g. `openspec`, `docs`) and `domain` documents for cross-package
clusters (see "the atlas has a domains axis"), plus the reserved `index.md` and
`log.md` files. Each concept's id (its bundle path) SHALL be qualified by the
unit's anchor path so that two units sharing a leaf name do not collide.

#### Scenario: indexing a repo writes the bundle
- **WHEN** `kenn index` completes on a repo with one or more internal packages
- **THEN** an atlas bundle exists at the `Layout`-resolved location containing
  `index.md`, `log.md`, and one concept document per internal package

#### Scenario: external dependency packages are excluded
- **WHEN** the repo's `packages` include external dependencies (e.g. `serde`)
- **THEN** no concept document is written for an external package

#### Scenario: foreign workspace writes under that workspace's store
- **WHEN** `kenn index -d ./tmp/other-repo` completes
- **THEN** the atlas bundle is written under `./tmp/other-repo`'s resolved store,
  not the invoking repo's, and its concepts describe `./tmp/other-repo`

### Requirement: concept documents are structural skeletons

Each package concept document SHALL carry YAML frontmatter with a non-empty
`type: package`, a `resource` naming the package manifest when present (else the
unit's directory path), and producer-defined `kenn.*` keys for kenn's structural
facts (at minimum: symbol count, dependency list, and the ranked central symbols).
All paths written inside the bundle (`resource`, member files, links) SHALL be
workspace-relative, never absolute. The concept document SHALL be deterministic and
SHALL NOT carry a per-concept wall-clock `timestamp` in v1, so re-indexing an
unchanged repo yields a no-op diff. Its body SHALL contain only structural content:
the package's most central symbols (ranked by weighted degree over the directed
weighted **aggregate graph** — `aggregate_nodes`/`aggregate_edges`, which is
already weighted and has its containers collapsed — excluding container kinds
(namespace/module/package); a production package excludes its test classes, while
a **test-dominant** package includes them and is tagged `tests`), its directed
dependencies as bundle-relative markdown links, and its top member files. Kenn
SHALL NOT write semantic prose ("what this is for") into concept bodies.

#### Scenario: package concept carries facts and a skeleton body
- **WHEN** a package concept document is generated
- **THEN** its frontmatter has `type: package` plus `kenn.*` fact keys, and its
  body lists central symbols, dependency links, and member files — with no
  kenn-authored interpretive prose

#### Scenario: description seeded verbatim from the root module doc
- **WHEN** the package's root module (selected by a language-keyed root-file rule)
  carries a module-level doc comment
- **THEN** the concept's `description` is that doc's text copied verbatim
- **WHEN** the package has no root module doc, or the root file is ambiguous
- **THEN** the concept's `description` is left empty for later enrichment

### Requirement: the atlas has a domains axis

Alongside the package axis, the atlas SHALL emit `domain` concept documents — one
per **cross-package cluster** — derived from the flat-Louvain communities the
analysis pass persists. The producer SHALL read those communities back from the
persisted analysis tables (`analysis_flat_communities`, `analysis_node_membership`)
and SHALL NOT recompute clustering nor depend on `kenn-analyze`; kenn-indexer and
kenn-analyze stay parallel consumers of the persisted graph. A community SHALL
become a domain only when it spans more than one package AND still spans more than
one package after its members are restricted to the domain-eligible set
(non-container, non-test, non-example, code + anchored) — a community that
collapses to a single package once containers, tests and example code are
excluded is the package concept's job. Each domain
SHALL be named by its hub (its highest-weighted-degree eligible member), carry that
hub's central list and its spanned packages as bundle-relative links, and render
`type: domain` with no `resource`. The bundle SHALL remain deterministic.

Example, sample, demo and fixture code SHALL be excluded from the domain-eligible
set for the same reason tests are: a bundled spike that references a library type
is not architecture. Every surface that computes domain eligibility — the producer
and any query over the published snapshot — SHALL take this fact from the
persisted aggregate node's `example` flag. No surface may re-derive it from paths,
and none may assume its absence.

#### Scenario: a cross-package cluster becomes a domain
- **WHEN** a persisted flat community spans two packages with enough eligible
  (non-container, non-test, non-example) members
- **THEN** a `domain` concept is written, named by its hub symbol, listing that
  hub's central members and links to the packages it spans

#### Scenario: a single-package community is not a domain
- **WHEN** a community's eligible members all resolve to one package (even if the
  raw community was flagged cross-anchor)
- **THEN** no domain concept is written — the package concept already covers it

#### Scenario: a span carried only by example code is not a domain
- **WHEN** a community spans two packages, but every one of its members in the
  second package is defined under an example/sample/demo/fixture path
- **THEN** the second package does not join the earned span, the community
  collapses to one package, and no domain concept is written

#### Scenario: the query and the atlas agree on eligibility
- **WHEN** the domains axis is read as a query over the same snapshot the atlas
  was rendered from, by the same build
- **THEN** it returns exactly the domains the atlas rendered, because both read
  the persisted `example` flag rather than deriving it

#### Scenario: domains need the analysis pass
- **WHEN** a repo is indexed with the analysis/clustering pass disabled
- **THEN** the package axis still emits, but no `domain` concepts are written
  (there are no persisted communities to read)

### Requirement: the bundle is OKF-conformant

Every non-reserved concept document SHALL have parseable YAML frontmatter with a
non-empty `type`. The reserved `index.md` and `log.md` SHALL carry NO YAML
frontmatter: `index.md` SHALL group concepts as a linked list under a markdown
header, and `log.md` SHALL record changes newest-first with ISO-8601 date
headings. When the bundle is regenerated on re-index, `log.md` SHALL be
append-preserved (a new dated section prepended, history retained) even as concept
documents and `index.md` are rewritten. Inter-package relationships SHALL be
expressed as standard bundle-relative markdown links between concept documents.

#### Scenario: bundle passes OKF conformance
- **WHEN** the bundle is validated against OKF v0.1
- **THEN** every concept document has a non-empty `type`, `index.md`/`log.md` carry
  no frontmatter, and both follow the reserved-file structure

#### Scenario: log.md history survives re-index
- **WHEN** a repo is indexed, then indexed again
- **THEN** `log.md` contains dated sections for both runs, newest first — the
  second index does not wipe the first run's entry

#### Scenario: a directed dependency is a link to another concept
- **WHEN** package A depends on package B (a directed import/use from A's symbols
  into B's, so the A→B direction is preserved — not the undirected rollup)
- **THEN** A's concept body contains a bundle-relative markdown link to B's
  concept document, and B's body does not spuriously link back to A for that edge

### Requirement: kenn index prints a markdown handle

On completion, `kenn index` SHALL announce the **published** atlas `index.md` path
(valid after the snapshot is published) on its existing output channel: in the
default (human) mode a single **marked** markdown line with a stable, greppable
prefix (e.g. `atlas: <path>`); under the existing `--json` mode a field on the
completion event. It SHALL NOT print a bare line into the `--json` stream, and an
agent SHALL be able to locate the atlas without parsing JSON in the default mode.
The `index.md` SHALL open with a frontmatter-free shape/status header stating at
least the languages, package count, symbol count, test ratio, a concrete freshness
signal (HEAD sha or `StalenessKey` + an ISO-8601 build timestamp), and the total
concept count.

#### Scenario: default mode announces the atlas with a greppable marker
- **WHEN** `kenn index` completes and publishes the snapshot (human mode)
- **THEN** its output includes a single marked line (stable prefix) naming the
  published atlas `index.md` path, locatable without parsing JSON

#### Scenario: --json mode carries the handle as a field, not a stray line
- **WHEN** `kenn index --json` completes
- **THEN** the atlas path is a field on the completion event and no bare markdown
  line is emitted into the JSON stream

#### Scenario: index.md carries the shape/status header
- **WHEN** the atlas `index.md` is read
- **THEN** its header states languages, package count, symbol count, test ratio, a
  concrete freshness signal (HEAD/staleness + timestamp), and total concept count

### Requirement: the producer runs on every indexing path

The atlas SHALL be emitted by a single shared finalize step, called by both the CLI
(`kenn index`) and the MCP orchestrated-indexing path after the run's code graph is
persisted, so that no entry point can publish an index without the atlas. The
producer SHALL read the run's persisted tables (packages, symbols, files, edges,
docs); its **package + document axes** SHALL NOT depend on whether the optional
analysis pass ran, while the **domains axis** reads the analysis tables the pass
persists (and is empty when the pass is disabled).

#### Scenario: CLI index emits the atlas
- **WHEN** a repo is indexed via `kenn index`
- **THEN** the atlas bundle is produced

#### Scenario: MCP orchestrated index emits the atlas
- **WHEN** a repo is indexed via an MCP orchestration path
- **THEN** the atlas bundle is produced by the same shared finalize step

#### Scenario: atlas emits regardless of the analysis pass
- **WHEN** a repo is indexed with the optional analysis/clustering pass disabled
- **THEN** the atlas bundle is still produced — the package + document concepts come
  from the raw graph; the domains axis is simply empty

### Requirement: a path-free consumption skill

Kenn's plugin SHALL provide a `skills/atlas/SKILL.md` whose steps derive the
atlas location from `kenn index` output rather than any hardcoded path, and whose
`description` carries orientation triggers so an agent reaches for it when
starting work in an unfamiliar or freshly-cloned repo.

#### Scenario: the skill contains no hardcoded atlas path
- **WHEN** the skill's steps are inspected
- **THEN** they obtain the atlas location from `kenn index` output and contain no
  literal bundle path

#### Scenario: the skill is discoverable by orientation intent
- **WHEN** a user asks to "understand this repo" / "get up to speed" / after a
  fresh clone
- **THEN** the skill's `description` matches that intent

### Requirement: the atlas has a contracts axis

Alongside the package and domains axes, the bundle SHALL emit a **contracts**
axis — `contract` concept documents plus a `## Contracts` section in `index.md` —
whose behavior is specified by the `atlas-contracts` capability. The bundle SHALL
write one `contracts/<slug>.md` per contract and include the contracts axis in the
`index.md` concept count. The contracts axis SHALL be additive: the existing
packages and domains axes are unchanged, and a bundle with no cross-package
contract writes no `## Contracts` section and no `contracts/` files.

#### Scenario: the bundle carries three axes

- **WHEN** a multi-package repo with cross-package interfaces is indexed
- **THEN** the bundle contains package concepts, domain concepts, and contract
  concepts, and `index.md` lists `## Domains` and `## Contracts` sections

#### Scenario: no contracts, no section

- **WHEN** a repo has no first-party interface implemented across package
  boundaries
- **THEN** the bundle writes no `contracts/` files and `index.md` has no
  `## Contracts` section (packages and domains are unaffected)

