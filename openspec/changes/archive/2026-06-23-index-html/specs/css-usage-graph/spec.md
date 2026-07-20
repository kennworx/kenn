## ADDED Requirements

### Requirement: HTML documents are a first-class class-usage source

The class-usage graph SHALL treat indexed HTML documents as first-class usage
sources. A `class=` attribute on an HTML element whose token intersects the class
registry SHALL produce a `uses_css_class` edge (attributed as defined by the
`html-index` capability — to the enclosing `html_id` element, else the HTML
document node). Class tokens appearing only in non-attribute positions
(comments, `<script>`/`<style>` text) SHALL NOT produce edges. Consequently
`check_css` SHALL NOT report a class as dead solely because its only usages are
in HTML — closing the false positive that exists while HTML is unindexed and its
mined edges are dropped for lack of a node.

#### Scenario: an HTML class usage lands as an edge

- **WHEN** `page.html` contains `<span class="btn">` and `btn` is in the registry
- **THEN** a `uses_css_class` edge is emitted from `page.html` (or its enclosing
  `html_id`) to the `btn` class node

#### Scenario: a class used only in HTML is not reported dead

- **WHEN** class `only-html` is defined in CSS and used only in `page.html`
- **THEN** `check_css` does not report `only-html` as an orphan/dead class

#### Scenario: a class in HTML comment text is not a usage

- **WHEN** `page.html` contains `<!-- <div class="ghost"> -->`
- **THEN** no `uses_css_class` edge is produced for `ghost`

### Requirement: the raw usage scan excludes indexed-HTML files

The language-agnostic `usage_sources` raw-text scan SHALL exclude files whose
extension is an indexed-HTML extension (`.html`/`.htm`), because the HTML parser
owns class-usage for those files. This prevents duplicate `uses_css_class` edges:
without the exclusion, once HTML files have document nodes the raw scan would stop
dropping their mined tokens (`css/ingest.rs:197`) and emit a second, crude edge
(comment- and `<script>`-polluted) alongside the parser's precise one. The
exclusion SHALL be unconditional, independent of `usage_sources` configuration.

#### Scenario: HTML is not double-scanned

- **WHEN** an HTML file's `class="btn"` is attributed by the HTML parser
- **AND** the same file would match a `usage_sources` glob
- **THEN** the raw scan emits no `uses_css_class` edge for that file (only the
  parser's edge exists)
