## MODIFIED Requirements

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
