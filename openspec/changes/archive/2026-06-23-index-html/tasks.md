# Tasks

Phased by tier per design.md. Phase 0 (keystone) gates everything; Phases 1–2
hang off the document/id nodes; Phase 3 (inline `<style>`) is independent;
Phase 4 wires the pipeline and verifies the cross-cutting effects. Parser:
**html5ever** (design D3); fall back to `swc_html_parser` only if the position
`TokenSink` proves painful.

## Phase 0 — parser, data model, discovery (keystone)

### 0. Dependency

- [x] 0.1 Add `html5ever` + `markup5ever_rcdom` to `kenn-indexer/Cargo.toml`. → verify: `cargo build -p kenn-indexer` succeeds; `cargo tree -p kenn-indexer | grep html5ever`.

### 1. Data model & identity (`kenn-model`)

- [x] 1.1 Add `Language::Html` (prefix `html`, `extensions(["html","htm"])`, no project files). → verify: prefix/db_name round-trip; both `.html` and `.htm` map to `Html`.
- [x] 1.2 Add `Kind::HtmlId`. → verify: `db_name`/`from_db_name` round-trip.
- [x] 1.3 Add the `id::html` native-ID builder `html:<relpath>#id:<name>` for `html_id`; the file node is `html:<relpath>` (a `document`). → verify: two ids in one file yield distinct node ids; an id and a same-named class never collide.

### 2. Parse & document node (`kenn-indexer/src/html/`)

- [x] 2.1 HTML discovery walker over configured roots (compile globs to `GlobSet`, apply the always-merged build-output excludes). → verify: discovery over a fixture tree finds `.html`/`.htm`, skips `dist/`.
- [x] 2.2 Parse each file with html5ever and emit a `document` node per file. → verify: a fixture `index.html` produces one document node with the right relpath.
- [x] 2.3 Position `TokenSink` (or DOM walk) that yields, per element, its attributes and a source **line**, plus parent nesting. → verify: element/attr lines match expected for a multi-line fixture.
- [x] 2.4 WHATWG quirk corpus tests (design spike list): void/open-only, valueless, unquoted, self-close, optional/implied close, raw-text, comments, dup attr, foreign, malformed, templating, case. → verify: each corpus case extracts the expected attrs/nesting; `<!-- class=x -->` and `<script>` strings yield no attribute.
- [x] 2.5 Line-granularity correctness under quirks: assert extracted lines stay correct across implied-close, raw-text, and multi-line attributes (the html5ever TokenSink/nesting-assembly risk in design D3). → verify: a fixture with `<li>`-implied-close and a multi-line `<script>` reports the right lines for downstream elements.

## Phase 1 — glue: links & imports

- [x] 3.1 Extract `<a href>` references and resolve via the markdown link resolver → `LinksTo`/`LinksToFile` with the existing grades. → verify: `<a href="b.html">` emits an edge to `b.html`; a missing target grades dangling.
- [x] 3.2 Extract `<link rel="stylesheet" href>` and `<script src>` → `Imports` edges (HTML→CSS, HTML→JS). → verify: fixture emits import edges to `app.css` and `app.js`.

## Phase 2 — ids, correspondence, assets

- [x] 4.1 Emit an `html_id` node per `id="…"`. → verify: `<div id="root">` yields an `html_id` node with the typed native id.
- [x] 4.2 Resolve `href="#frag"` / `href="page#frag"` to the target `html_id` — reuse the resolver's file/path resolution + grades, but look the fragment up in the target file's `html_id` anchors (not markdown sections). → verify: `<a href="#intro">` resolves to the `intro` `html_id`; unknown fragment grades dangling.
- [x] 4.3 Emit `CorrespondsTo` between an `html_id` and a same-named `css_id` when both exist; lone `html_id` (React mount) stays uncorresponded. → verify: matching ids correspond; `<div id="root">` with no `#root` selector is a lone node.
- [x] 4.4 Resolve asset refs (`<img>`/`<video>`/`<source>`/`<iframe>` and `<a href>` to a non-indexed file) → `LinksTo` edges to `attachment` **stub** nodes (symbol-space, not the files table — design D7). Key each stub by the **canonical workspace-relative path** (`src`/`href` resolved relative to the file + normalized) so all spellings collapse to one stub; missing asset → dangling stub keyed by the written string. → verify: `<img src="logo.png">` → a `LinksTo` edge to a path-keyed stub; `../logo.png` from two files → the **same** stub; a missing asset grades dangling.

## Phase 3 — embedded inline `<style>` (Tier 3)

- [x] 5.1 Extract `<style>` block text, feed it to the existing CSS extractor, and rebase positions by the block's base offset so defs land on the right HTML lines. Nodes reuse the shared `Kind::CssClass`/`CssId`/`CssVar` with `html:<relpath>#<type>:<name>` native ids, registered into the shared class registry (design D6). → verify: `<style>.hero{}</style>` defines `css_class` `html:page.html#class:hero` at its HTML line, and a `class="hero"` elsewhere resolves to it.
- [x] 5.2 Confirm inline `<script>`, event handlers, and inline `style=` are NOT indexed (out of scope). → verify: `<script>const x=1</script>` extracts no JS symbols.

## Phase 4 — class usage attribution, pipeline, cross-cutting verification

- [x] 6.1 Attribute `class=` usages (real attributes only) against the CSS class registry → `uses_css_class` from the enclosing `html_id`, else the document node. → verify: `<span class="btn">` emits an edge; `<div id="card"><span class="btn">` attributes to `card`; an unregistered token emits nothing.
- [x] 6.1a Exclude indexed-HTML extensions (`.html`/`.htm`) from the language-agnostic `usage_sources` raw scan, so the parser path is the sole source of HTML class usage (design D5 — prevents double edges). → verify: an HTML file matching a `usage_sources` glob produces exactly one `uses_css_class` edge per real `class=` token (no raw-scan duplicate; no comment/`<script>` phantom).
- [x] 6.2 Wire HTML ingest as a parallel producer; gate link/correspondence/usage resolution on the post-code + CSS-registry barrier. → verify: orchestration test shows usage/correspondence run after code + CSS registry complete.
- [x] 6.3 Confirm the keystone fix: a class used **only** in HTML is no longer reported dead by `check_css`. → verify: a fixture where `.only-html` appears only in `page.html` — `check_css` does not flag it.
- [x] 6.4 End-to-end index of an HTML+CSS+JS fixture; assert the document/id nodes, link/import/usage/correspondence edges, and inline-style defs all resolve. → verify: e2e test green; `cargo clippy --workspace --all-targets` clean; `just crap-ci` passes.
