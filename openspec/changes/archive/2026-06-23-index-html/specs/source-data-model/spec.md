## ADDED Requirements

### Requirement: HTML is a modelled language with document and html_id nodes

The public-ID scheme SHALL gain an `html:` language prefix covering `.html` and
`.htm` files. Each HTML file SHALL be modelled as a `document` node (mirroring
markdown documents), and each `id="…"` attribute SHALL define an `html_id` node
with a typed native-ID form `html:<relpath>#id:<name>` (the `#id:` type segment
keeps it distinct from any other node namespace for the same file). HTML SHALL
reuse existing edge kinds — `LinksTo`, `LinksToFile`, `Imports`, `UsesCssClass`,
and `CorrespondsTo` — and SHALL NOT introduce a new edge kind. The edge for a
reference is chosen by its target's table: an indexed-file target uses
`LinksToFile`/`Imports`; a node or attachment-stub target uses `LinksTo`
(asset stubs included — HTML does not use the transclusion edge `Embeds`). The
HTML-id ↔ CSS-id relation reuses `CorrespondsTo`, not a usage edge.

#### Scenario: an html file gets the html prefix and a document node

- **WHEN** `pages/index.html` is indexed
- **THEN** it is modelled as a `document` node under the `html:` prefix

#### Scenario: an id attribute gets a typed html_id native id

- **WHEN** `pages/index.html` contains `<div id="root">`
- **THEN** an `html_id` node with native id `html:pages/index.html#id:root` exists

#### Scenario: no new edge kind is introduced for HTML

- **WHEN** HTML links, imports, asset refs, class usage, and id correspondence are emitted
- **THEN** they use only the existing `LinksTo`/`LinksToFile`/`Imports`/
  `UsesCssClass`/`CorrespondsTo` edge kinds

#### Scenario: an inline-style css node is owned by the HTML file

- **WHEN** `page.html` defines `.hero` in an inline `<style>` block
- **THEN** the node reuses `Kind::CssClass` with native id `css:page.html#class:hero`
  (the `css:` prefix marks it a CSS node, the HTML relpath records the owner; the
  kind stays shared, and it stays distinct from the `html_id` `#id:` namespace)
