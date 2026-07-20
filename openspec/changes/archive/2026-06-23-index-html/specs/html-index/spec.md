## ADDED Requirements

### Requirement: HTML files are discovered and indexed as document nodes

The indexer SHALL discover `.html`/`.htm` files under the configured roots and
model each as a `document` node in the graph, parsed with a WHATWG-conformant
HTML parser. This is the keystone: once an HTML file is a node, usage and link
edges have a source to attach to. The document node SHALL carry the file's
workspace-relative path and SHALL be a navigable graph node like a markdown
document.

#### Scenario: an HTML file becomes a document node

- **WHEN** the workspace contains `pages/index.html` under an indexed root
- **THEN** the graph contains a `document` node for `pages/index.html`

#### Scenario: htm extension is indexed

- **WHEN** a file `legacy.htm` exists under an indexed root
- **THEN** it is indexed as an HTML document node

### Requirement: HTML parsing handles WHATWG quirks at line granularity

The parser SHALL correctly handle the WHATWG quirk set — void/open-only tags
(`<br>`, `<img>`, `<input>`), valueless attributes (`disabled`), unquoted
attribute values, self-closing slashes on non-void elements (ignored), optional
and implied closing tags (`<li>a<li>b` are siblings), and raw-text elements
(`<script>`/`<style>` content is not parsed as markup). Extracted nodes and
edges SHALL carry a source **line** (not a byte offset); the graph surface is
line-based.

#### Scenario: a void tag does not swallow following siblings

- **WHEN** an HTML file contains `<div><br><a href="/x">y</a></div>`
- **THEN** the `<a>` is a sibling of `<br>`, not its child

#### Scenario: a class in a comment produces no usage

- **WHEN** an HTML file contains `<!-- <div class="ghost"> -->`
- **THEN** no usage is attributed for `ghost` (extraction is scoped to real
  element attributes)

#### Scenario: raw-text script content is not parsed as markup

- **WHEN** an HTML file contains `<script>if (a < b) {}</script>`
- **THEN** `< b` is treated as script text, not an element

### Requirement: HTML link references resolve to graph edges

The indexer SHALL resolve `<a href>` references to link edges, reusing the
markdown link resolver's **file/path resolution and grading**: a reference to a
file yields `LinksToFile`, a reference to another document yields `LinksTo`. A
fragment (`href="#frag"` or `href="page#frag"`) SHALL resolve against the target
file's `html_id` anchors — the HTML analog of markdown's section anchors, not the
markdown section table — yielding `LinksTo` to that `html_id`. Unresolved
references SHALL be graded dangling rather than dropped, matching the markdown
link grades.

#### Scenario: an href to another document is a link edge

- **WHEN** `a.html` contains `<a href="b.html">`
- **THEN** a `LinksTo`/`LinksToFile` edge is emitted from `a.html` to `b.html`

#### Scenario: a fragment href resolves to an html_id

- **WHEN** `a.html` contains `<a href="#intro">` and an element `id="intro"`
- **THEN** a `LinksTo` edge resolves to the `intro` `html_id` node

### Requirement: stylesheet and script references become import edges

The indexer SHALL emit an `Imports` edge from the HTML document to the
referenced file for `<link rel="stylesheet" href>` (HTML → CSS) and
`<script src>` (HTML → JS/TS), resolved against the workspace file set. A
reference to a file not in the workspace SHALL be graded dangling, not dropped.

#### Scenario: a stylesheet link is an import edge

- **WHEN** `index.html` contains `<link rel="stylesheet" href="app.css">`
- **THEN** an `Imports` edge is emitted from `index.html` to `app.css`

#### Scenario: a script src is an import edge

- **WHEN** `index.html` contains `<script src="app.js">`
- **THEN** an `Imports` edge is emitted from `index.html` to `app.js`

### Requirement: id attributes define html_id nodes that correspond to css ids

An `id="…"` attribute SHALL define an `html_id` node owned by the HTML file; the
HTML file owns the id definition. When a `css_id` node of the same name exists,
the indexer SHALL join them with a `CorrespondsTo` edge. There SHALL be no
`uses_css_id` edge — the relation between an HTML id and a CSS `#id` selector is
correspondence, not usage. An `html_id` with no matching `css_id` (e.g. a JS
mount point) is a valid lone node.

#### Scenario: an id defines an html_id node

- **WHEN** an HTML file contains `<div id="root">`
- **THEN** an `html_id` node `root` is defined for that file

#### Scenario: matching html and css ids correspond

- **WHEN** an `html_id` `header` exists and a `css_id` `header` exists
- **THEN** a `CorrespondsTo` edge joins them

#### Scenario: a JS mount id is a lone node

- **WHEN** `<div id="root">` exists but no CSS `#root` selector exists
- **THEN** the `root` `html_id` node exists with no correspondence edge

### Requirement: class usage is attributed to the enclosing id'd element or document

The indexer SHALL emit `uses_css_class` edges for `class=` attributes that
intersect the CSS class registry, attributed to the **nearest** enclosing id'd
element's `html_id` node when present, otherwise to the HTML document node. Extraction
SHALL be scoped to real element attributes (not comments or `<script>`/`<style>`
text). A class token that does not intersect the registry SHALL NOT produce an
edge or a node. Because HTML files are now graph nodes, the language-agnostic raw
usage scan (`css-usage-graph`) SHALL exclude indexed-HTML extensions, so HTML
class usage comes **only** from this parser path — there SHALL NOT be duplicate
`uses_css_class` edges for the same HTML attribute.

#### Scenario: a class attribute becomes a usage edge

- **WHEN** an HTML file contains `<span class="btn">` and `btn` is in the registry
- **THEN** a `uses_css_class` edge is emitted to the `btn` class node

#### Scenario: usage attributes to the enclosing id'd element

- **WHEN** `<div id="card"><span class="btn"></div>` and `btn` is in the registry
- **THEN** the `uses_css_class` edge source is the `card` `html_id` node

#### Scenario: no duplicate edge from the raw usage scan

- **WHEN** an HTML file's `class="btn"` is attributed by the HTML parser
- **THEN** the raw `usage_sources` scan does not also emit a `uses_css_class` edge
  for the same attribute (indexed-HTML extensions are excluded from that scan)

### Requirement: asset references become attachment nodes via LinksTo

The indexer SHALL resolve references to non-indexed assets — `<img>`/`<video>`/
`<source>`/`<iframe>` `src`, and `<a href>` pointing at a non-indexed file — to an
`attachment` node via a `LinksTo` edge, reusing the markdown attachment model. An
`attachment` is a leaf **stub node in the symbol space** (kenn does not index
binary assets), so the symbol-targeting `LinksTo` is the correct edge — **not**
the file-table `LinksToFile`, which is reserved for references that resolve to an
indexed file. HTML does not distinguish transclusion from reference at the graph
level. Edges carry the standard link grades (dangling when the asset is absent).

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

### Requirement: inline style blocks are routed through the CSS extractor

The indexer SHALL extract inline `<style>` blocks by feeding the block's text to
the existing CSS extractor, rebasing the extractor's positions by the block's
base offset so definitions resolve to the correct lines in the HTML file. Nodes
defined by an inline block SHALL reuse the shared CSS node kinds
(`css_class`/`css_id`/`css_var`) under the **`css:` prefix** with the HTML file as
relpath (`css:<relpath>#class:<name>`, etc.) — distinct from the file's `html_id`
ids (`html:<relpath>#id:<name>`) so an inline `#id` selector corresponds to the
matching element instead of colliding — and SHALL register into the **same shared
class registry** so `class=` usages elsewhere can resolve to an inline-defined
class. Inline `<script>` blocks, event-handler attributes, and
inline `style=` declarations SHALL NOT be indexed (they require the separate
JS/TS pipeline and are out of scope).

#### Scenario: an inline style block defines a registered css node owned by the HTML file

- **WHEN** an HTML file `page.html` contains `<style>.hero { }</style>`
- **THEN** a `css_class` node with native id `css:page.html#class:hero` is
  defined at its line, registered in the shared class registry

#### Scenario: an inline script is not indexed

- **WHEN** an HTML file contains `<script>const x = 1;</script>`
- **THEN** no JS symbols are extracted from it
