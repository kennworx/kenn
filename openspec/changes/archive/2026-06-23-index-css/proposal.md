## Why

CSS/SCSS is invisible to kenn today. A class like `.btn-primary` is *defined* in
a stylesheet and *used* across components as bare string literals
(`className="btn-primary"`, `class="card"`, `clsx('btn', …)`, `styles.btnPrimary`)
— and those string usages are invisible to the SCIP indexers, which have no
concept of a CSS class. The result: you cannot answer "where is this class
used?", "which stylesheet defines this class?", or "is this CSS dead?". These are
exactly the questions a code-graph should answer, and kenn already owns the
machinery — a typed node/edge graph, FTS5 + embeddings, the navigation tools,
and a recall-first resolution ladder built for markdown link-rot. The marginal
cost is a stylesheet producer plus a usage-mining pass; everything downstream is
reused.

The linking model is borrowed from **Tailwind's content scanner**: treat source
files as raw text, extract class-shaped candidates with a broad tokenizer, and
keep only those that intersect a **known class registry**. The registry (built
from parsed CSS/SCSS) is what makes recall-first extraction safe — a stray
string only becomes an edge if a matching class actually exists.

## What Changes

- Index stylesheets as a first-class corpus in the **same unified graph** as
  code: `css:`/`sass:` nodes (classes, ids, custom properties) sit beside
  `rs:`/`ts:`/… nodes in one store. Class/id/var prose (selector text, adjacent
  comments) feeds FTS5 + embeddings.
- **A class registry** — the deduplicated set of atomic class names with their
  definition sites (file + byte range). This is both a queryable artifact and
  the "known set" the usage scanner intersects against.
- **Usage mining across all configured languages** (Tailwind-style): scan files
  matched by configurable **`usage_sources` globs** as raw text, extract class-shaped
  candidates, intersect with the registry, emit `uses_css_class` edges from the
  code file to the class node. Language-agnostic by construction — new file
  types are a glob, not a parser.
- **CSS-internal graph**: `@import`/`@use`/`@forward` (module→module) and
  `@extend .class`/`composes` (rule→class) become edges, enabling "what does this
  stylesheet depend on?" and dead-stylesheet detection. These at-rules are
  resolved away by compilation, so they come from a **light source scan** (keyword
  spotting, not full Sass parsing). v1 targets only existing node kinds (class,
  module); `@extend %placeholder` and `@include`/mixin edges (which would need
  placeholder/mixin node kinds) are deferred.
- **Recall-first resolution with grading, edges only on a hit**: a usage inside
  a recognized `class=`/`className=`/module-member context grades `Exact`; a bare
  matching token elsewhere grades `Fuzzy` (`Ambiguous` when several definitions
  share the name). A token that does **not** match the registry produces **no
  edge and no node** — so Tailwind utilities (which have no definition) don't
  inflate the graph; undefined tokens are surfaced only by the `check_css`
  report (under a utility allowlist), never as dangling stubs.
- **Orphan / hygiene reporting** (`check_css`, cousin of `check_links`):
  surfaces orphaned classes (defined, zero usages → dead CSS), orphaned
  stylesheets (no used selectors and not imported), and dangling code classes
  (used, no definition, not a known utility → likely typo/missing style).
- **Parser strategy split by file type** — see design.md for the spike that
  drove this: **lightningcss** for `.css` (typed, atomic, error-tolerant; proven
  flawless across 162 real files); for `.scss`/`.sass` the **dart-sass compiler
  is required** — it compiles to CSS (the only path that captures
  `@each`/mixin-**generated** classes, validated on Bootstrap+Bulma with
  source-map back-mapping to source:line) → lightningcss extract. dart-sass
  handles both `.scss` and indented `.sass` natively. **No bespoke fallback
  parser**: an entry that fails to compile is skipped + logged (a hand-rolled
  Sass scanner would be lossy and subtly wrong — the spike showed even real
  grammars/compilers get Sass wrong). The compiler is discovered from natural
  project locations (`node_modules/.bin`, the `sass`/`sass-embedded` npm
  packages, `PATH`) or bundled like the other sidecars. Integration is via the
  stable `sass` CLI, not the Rust `sass-embedded` crate (proven protocol-stale
  against current dart-sass).
- Build runs as a **sibling producer** (design D1, like markdown), **in
  parallel** with code ingest; usage mining and orphan reporting wait on a
  post-code **join barrier**. No incremental machinery — the snapshot rebuild
  re-resolves the whole corpus each run.

## Phasing

This change is scoped in three phases so value lands incrementally and the
SCSS-parser decision can be validated before the most code is written.

- **Phase 1 — parse + class registry + definition sites.** lightningcss for
  `.css`; dart-sass CLI compile + source-map → lightningcss for `.scss`/`.sass`
  (compiler required; failed entries skipped + logged). Classes/ids/custom-props
  become searchable nodes with locations. Delivers: search/embeddings over the
  stylesheet corpus, and the registry (incl. generated classes) later phases
  depend on.
- **Phase 2 — usage mining + the code↔class graph.** Tailwind-style usage scan
  over configured `usage_sources`, `uses_css_class` edges with grading, CSS-Modules
  binding-aware resolution (JS/TS sources). Delivers: "where is `.btn` used?",
  "which CSS styles this?", code→class backlinks.
- **Phase 3 — CSS-internal graph + orphan/hygiene reports.** `@import`/`@use`/
  `@extend`/`composes` edges, `check_css` orphan/dangling reporting. Delivers:
  dead-CSS and dead-stylesheet detection, the `&`-resolved selector graph.

## Capabilities

### New Capabilities

- `css-index`: stylesheets as an indexed corpus — glob discovery, the parser
  split (lightningcss for `.css`; dart-sass compiler for `.scss`/`.sass`), atomic
  class/id/custom-property extraction with `&`-nesting resolution and definition
  sites, the class registry, and selector/comment prose flowing into FTS5 +
  embeddings. Runs as a sibling producer parallel to code ingest.
- `css-usage-graph`: the usage and dependency edges and their resolution — the
  Tailwind-style usage scan intersected with the registry, `uses_css_class` edges
  with recall-first grading, CSS-Modules binding-aware resolution (JS/TS), the
  CSS-internal `@import`/`@use`/`@extend`/`composes` edges, and the `check_css`
  orphan/dangling hygiene report. Usage resolution is gated behind the post-code
  barrier.

### Modified Capabilities

- `source-data-model`: extend the public-ID scheme with **two** stylesheet
  language prefixes — `css:` (`.css`) and `sass:` (`.scss`/`.sass`, one language
  two syntaxes) — and a **typed** native-ID form
  `<lang>:<relpath>#<type>:<name>` (type ∈ class/id/var, so `.hero` and `#hero`
  never collide); model each stylesheet file as a `module` node owning its
  selectors; add `uses_css_class` and `extends_rule` edge kinds and reuse `imports`
  (module→module) for `@use`/`@import`; add `css_class` / `css_id` / `css_var`
  node kinds **shared** across both languages. The split is source-provenance
  metadata; the node kinds and the class registry stay unified.
- `indexing-orchestrator`: stylesheet ingest runs as an additional parallel
  ingest unit during the ingest phase; usage mining + CSS-internal resolution +
  orphan reporting run as a step gated on completion of all code ingest units,
  before finalize/publish.
