## MODIFIED Requirements

### Requirement: HTML link references resolve to graph edges

The indexer SHALL resolve `<a href>` references to link edges, reusing the
markdown link resolver's **file/path resolution and grading**: a reference to a
file yields `LinksToFile`, a reference to another document yields `LinksTo`. A
fragment (`href="#frag"` or `href="page#frag"`) SHALL resolve against the target
file's `html_id` anchors — the HTML analog of markdown's section anchors, not the
markdown section table — yielding `LinksTo` to that `html_id`. Unresolved
references SHALL be graded dangling rather than dropped, matching the markdown
link grades.

Because the grading reuse is delegation rather than duplication, an `href` SHALL
be joined onto the linking file's directory by the **same** rule the markdown
resolver uses — including root-relative (`/…`) hrefs and `..` segments that walk
above the workspace root — and a single `href` SHALL NOT be resolved by one rule
for its fragment and a different rule for its file target.

#### Scenario: an href to another document is a link edge

- **WHEN** `a.html` contains `<a href="b.html">`
- **THEN** a `LinksTo`/`LinksToFile` edge is emitted from `a.html` to `b.html`

#### Scenario: a fragment href resolves to an html_id

- **WHEN** `a.html` contains `<a href="#intro">` and an element `id="intro"`
- **THEN** a `LinksTo` edge resolves to the `intro` `html_id` node

#### Scenario: a relative href grades against the joined path

- **WHEN** `site/pages/a.html` contains `<a href="../b.html">` and the file
  exists at `site/b.html`
- **THEN** the edge targets `site/b.html` and is graded exact

#### Scenario: a fragment href joins by the same rule as a file href

- **WHEN** `site/pages/a.html` contains `<a href="../b.html#intro">` and
  `site/b.html` declares `id="intro"`
- **THEN** the `html_id` lookup resolves against `site/b.html` — the same joined
  path a file href would produce, not a differently-normalized one

#### Scenario: an href above the workspace root does not resolve

- **WHEN** an href's `..` segments walk above the workspace root
- **THEN** the reference SHALL NOT resolve to an in-workspace file
- **AND** it is graded dangling rather than resolving against the workspace root

### Requirement: asset references become attachment nodes via LinksTo

The indexer SHALL resolve references to non-indexed assets — `<img>`/`<video>`/
`<source>`/`<iframe>` `src`, and `<a href>` pointing at a non-indexed file — to an
`attachment` node via a `LinksTo` edge, reusing the markdown attachment model. An
`attachment` is a leaf **stub node in the symbol space** (kenn does not index
binary assets), so the symbol-targeting `LinksTo` is the correct edge — **not**
the file-table `LinksToFile`, which is reserved for references that resolve to an
indexed file. HTML does not distinguish transclusion from reference at the graph
level. Edges carry the standard link grades (dangling when the asset is absent).

Eligibility SHALL be decided by **existence in the workspace**, not by the
target's spelling. A reference whose target has no file extension, and one
naming a directory, SHALL be eligible on the same terms as one carrying a known
asset extension; the existence lookup, not an extension test, decides whether an
attachment is minted. The lookup SHALL report a directory as existing.

The attachment stub SHALL be keyed by a **canonical workspace-relative path** —
the `src`/`href` resolved relative to the referencing file and normalized — so
that every spelling of the same asset (`logo.png`, `../logo.png`,
`/assets/logo.png`) collapses to a **single** stub node. This is what makes
reverse lookup deterministic: an agent holding the on-disk path can compute the
same key and find every reference (the `find-usages` change depends on this).
A reference whose target does not exist on disk falls back to a dangling stub
keyed by the written string.

#### Scenario: an image reference is a LinksTo to a path-keyed attachment stub

- **WHEN** an HTML file contains `<img src="logo.png">`
- **THEN** a `LinksTo` edge resolves to an `attachment` stub keyed by the
  canonical workspace-relative path of `logo.png`

#### Scenario: different spellings collapse to one stub

- **WHEN** `pages/a.html` has `<img src="../logo.png">` and `pages/b.html` has
  `<img src="../logo.png">` resolving to the same on-disk asset
- **THEN** both edges target the **same** attachment stub node (so `find_usages`
  on that asset returns both references)

#### Scenario: an extensionless href that exists becomes an attachment

- **WHEN** an HTML file contains `<a href="LICENSE-MIT">` and that file exists
- **THEN** a `LinksTo` edge resolves to an `attachment` stub keyed by
  `LICENSE-MIT`
- **AND** the edge is graded exact rather than dangling

#### Scenario: an href naming a directory that exists becomes an attachment

- **WHEN** an HTML file contains `<a href="docs/">` and that directory exists
- **THEN** a `LinksTo` edge resolves to an `attachment` stub keyed by `docs`

#### Scenario: an extensionless href that does not exist still dangles

- **WHEN** an HTML file contains `<a href="about">` and nothing exists at that
  path
- **THEN** the edge targets a stub keyed by the written string and is graded
  dangling
