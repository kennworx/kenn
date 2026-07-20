## ADDED Requirements

### Requirement: HTML ingest runs as a parallel producer gated for connective resolution

HTML ingest SHALL run as an additional parallel producer during the ingest phase
(alongside code, markdown, and stylesheet ingest). Its connective steps —
`<a href>`/fragment link resolution, `html_id`↔`css_id` correspondence, and
`class=`/`id=` usage attribution — SHALL run as a step gated on completion of
code ingest and the CSS class registry, mirroring how stylesheet usage
resolution is gated: the code file nodes and the class/id registries must exist
before HTML edges can resolve against them. The gated step SHALL run before
finalize/publish.

#### Scenario: document nodes are produced in the parallel phase

- **WHEN** the ingest phase runs
- **THEN** HTML document nodes are produced in parallel with code/CSS ingest

#### Scenario: class usage resolution waits for the registry

- **WHEN** HTML `class=` usage attribution runs
- **THEN** it runs only after code ingest and the CSS class registry are complete

#### Scenario: id correspondence waits for css ids

- **WHEN** `html_id`↔`css_id` correspondence is computed
- **THEN** it runs after the CSS id nodes have been produced
