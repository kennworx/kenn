## Why

kenn indexes code, markdown, and (as of `index-css`) stylesheets — but HTML, the
markup that *connects* those islands, is invisible. An HTML page is where CSS
classes are actually used, where `<link>`/`<script>` wire a document to its
stylesheet and scripts, where `<a href>` links resolve between documents, and
where the ids that JS and CSS reference are defined. Today none of that reaches
the graph.

The gap is already biting: `index-css`'s class-usage scan walks **any** file and
mines `class=` (HTML's own attribute), but drops the edge for any HTML file
because HTML isn't an indexed language and so has no node to attach to
(`css/ingest.rs:197`). As a direct consequence, `index-css`'s new `check_css`
dead-code report **flags classes used only in HTML as dead** — a false positive.
Making HTML a first-class document gives those usages a node to attach to; the
HTML parser then produces them *precisely* (scoped to real attributes) and the
raw scan steps aside for HTML, fixing that report without double-counting (see
design D5).

## What Changes

HTML's value in the graph is overwhelmingly **edges, not symbols** — it
contributes almost no callable code; it connects CSS, JS, and documents. The
change adds HTML as an indexed language and wires those connections, in tiers:

- **Keystone** — add `Language::Html` (prefix `html`, extensions `html`/`htm`)
  and model each HTML file as a `document` node. This alone activates the
  class-usage edges that are already mined-then-dropped, and fixes the
  `check_css` false positive.
- **Glue** (reuse existing resolvers) — `<a href>` → `LinksTo`/`LinksToFile`
  via the markdown link resolver; `<link rel=stylesheet>` / `<script src>` →
  `Imports` (HTML → CSS / JS); `class=` usage attributed through the existing
  CSS class registry.
- **IDs & assets** — `id="…"` defines a new `html_id` node (HTML **owns** the
  id; CSS `#sel` and JS references *correspond to* it via `corresponds_to`, with
  no `uses_css_id` edge); `href="#frag"` resolves to it; `<img src>` and other
  asset refs become `LinksToFile`/`attachment` nodes.
- **Embedded CSS** (Tier 3) — inline `<style>` blocks routed through the
  existing CSS extractor (offset-rebased into the HTML file). Inline `<script>`
  is **out of scope** (it requires the separate JS/TS indexer pipeline).

Parsing uses **html5ever** (decision recorded in `design.md`, backed by a
measured parser spike) for full WHATWG quirk handling — void/open-only tags,
valueless/unquoted attributes, optional/implied close tags, raw-text
`<script>`/`<style>` — which a raw-text scan cannot do correctly. A real parse
also raises class-usage **precision**: `class=` inside comments or `<script>`
strings stops producing phantom edges.

## Capabilities

### New Capabilities

- `html-index`: HTML as an indexed corpus — glob discovery, the html5ever parse,
  `document` nodes per file, `html_id` anchor nodes, `<a href>`/`href="#frag"`
  link edges (reusing the markdown link resolver), `<link>`/`<script>` import
  edges, `class=`/`id=` usage attribution scoped to real attributes, `attachment`
  nodes for asset refs, `corresponds_to` between `html_id` and `css_id`, and
  (Tier 3) inline `<style>` routed through the CSS extractor. Runs as a sibling
  producer; connective resolution is gated behind the post-code/CSS barrier.

### Modified Capabilities

- `source-data-model`: add the `html:` language prefix (`.html`/`.htm`) and model
  each HTML file as a `document` node; add an `html_id` node kind with a typed
  native-ID form (`html:<relpath>#id:<name>`); reuse existing edge kinds
  (`LinksTo`/`LinksToFile`/`Imports`/`UsesCssClass`/`CorrespondsTo`) — no new edge
  kind (ids use correspondence, not a `uses_css_id` edge; assets are `LinksTo` to
  an attachment stub, the edge chosen by the target's table — see design D7).
- `css-usage-graph`: HTML documents become a first-class class-usage source —
  `class=` edges already mined from HTML now **land** (HTML files gain a node),
  extraction is scoped to real attributes (dropping comment/`<script>`-string
  false positives), and `check_css` no longer reports HTML-only classes as dead.
- `indexing-orchestrator`: HTML ingest runs as an additional parallel producer
  during the ingest phase; link resolution, `html_id`↔`css_id` correspondence,
  and class/id usage attribution run as a step gated on completion of code and
  CSS-registry ingest, before finalize/publish.

## Impact

- **Depends on `index-css`:** this change reuses the CSS class registry and
  **modifies** the `usage_sources` raw scan (to exclude indexed-HTML). Land it
  after `index-css` is merged.
- **New dependency:** `html5ever` + `markup5ever_rcdom` (measured +549 KB to the
  binary, 24 transitive crates — see the spike in `design.md`). Pure-Rust,
  consistent with the `pulldown-cmark`/`lightningcss` house style.
- **Behavior:** HTML pages become navigable nodes; CSS/JS/document links that
  were invisible now resolve; the `index-css` dead-code report gains precision.
- **Surface:** new `html-index` ingest unit under `crates/kenn-indexer/src/html/`,
  one `Language` variant, one node kind, an `id::html` scheme; no MCP tool
  changes (HTML nodes/edges flow through the existing graph tools).
- **Scoping:** inline `<script>` and event-handler/inline-`style=` JS are
  deferred (separate-pipeline cost); noted in `design.md` open questions.
