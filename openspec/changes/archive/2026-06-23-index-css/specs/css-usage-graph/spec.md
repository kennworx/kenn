<!-- Cross-reference: the `index-html` change makes HTML a first-class usage
source. HTML files are scanned today but their `uses_css_class` edges are
dropped for lack of a node (`css/ingest.rs:197`); once HTML files are `document`
nodes those edges land, a real HTML parse scopes `class=` to true attributes
(dropping comment/`<script>`-string false positives), and `check_css` stops
reporting HTML-only classes as dead. The `index-html` change records this as an
ADDED `css-usage-graph` delta (plus the `html-index` capability); both merge into
this spec's baseline when the changes archive. See `openspec/changes/index-html/`. -->

## ADDED Requirements

### Requirement: Class usage is mined from source text across configured globs

The indexer SHALL mine class usages by scanning files matched by configurable
**`usage_sources` globs** as raw text (independent of which code languages are
SCIP-indexed), extracting class-shaped candidate tokens, and keeping only those
that intersect the class registry. For each kept candidate it SHALL emit a
`uses_css_class` edge from the **enclosing code symbol when resolvable, otherwise
the file's containing module node, otherwise the file node**, to the class node.
The fallback is load-bearing, not a corner case: some code indexers emit
declaration-line-only def ranges, so a usage in a function *body* resolves to no
enclosing symbol — without the fallback the usage graph would silently drop on
real code. A candidate that does not intersect the
registry SHALL NOT produce an edge or a node (no dangling stubs); undefined
class-shaped tokens are surfaced only by the `check_css` report.

#### Scenario: A className string becomes a usage edge

- **WHEN** a `.tsx` file under a `usage_sources` glob contains `className="btn card"`
- **AND** classes `btn` and `card` exist in the registry
- **THEN** `uses_css_class` edges are emitted from that file to both class nodes

#### Scenario: A non-matching string is not an edge

- **WHEN** a source string `"the"` has no matching class in the registry
- **THEN** no `uses_css_class` edge is produced for it

#### Scenario: Usage scanning is language-agnostic

- **WHEN** a `usage_sources` glob matches a file type with no kenn code indexer (e.g.
  `.html`, `.vue`)
- **THEN** class usages in that file are still mined

#### Scenario: A body-level usage falls back to the module node

- **WHEN** a class is used in a function body but the code indexer recorded only
  the function's declaration line (no symbol covers the usage line)
- **THEN** the `uses_css_class` edge still emits, sourced from the file's module
  node rather than being dropped

### Requirement: Usage edges are graded by context

Each `uses_css_class` edge SHALL carry a confidence grade reflecting *how* the token
was found (every edge is a registry hit, so it always has a real target). A match
within a recognized class-attribute context (`class=` / `className=`, or a
CSS-module member access) SHALL grade `Exact`; a bare matching token elsewhere in
the file SHALL grade `Fuzzy` (or `Ambiguous` when several registry definitions
share the name). The grade SHALL NOT model "undefined class" — unmatched tokens
produce no edge at all.

#### Scenario: Attribute-context match grades Exact, bare token grades Fuzzy

- **WHEN** a registry class `card` appears once in `className="card"` and once as
  a bare string `"card"` elsewhere in the same file
- **THEN** the first usage edge grades `Exact` and the second `Fuzzy`

#### Scenario: Tailwind utilities produce no edges

- **WHEN** a file uses utility classes (e.g. `flex`, `pt-4`) absent from every
  indexed stylesheet
- **THEN** no `uses_css_class` edges and no stub nodes are created for them
- **AND** they are surfaced (if at all) only by the `check_css` report under the
  utility allowlist

### Requirement: CSS Modules usage resolves to the imported stylesheet

kenn SHALL resolve CSS-module member references to a class in the bound
stylesheet specifically. Unlike the language-agnostic token scan, this requires
**import-binding analysis** and is therefore scoped to **JS/TS `usage_sources`**:
the resolver maps the import local name to the imported `*.module.css` file (it
MAY reuse the existing code import edges in the store) and then a member access on
that binding to a class in that file, applying a camelCase↔kebab-case fold
(`btnPrimary` ↔ `btn-primary`). This binding-anchored resolution SHALL take
precedence over the global token scan for those references. Non-JS sources
(`.html`, `.vue` templates, etc.) have no module binding and fall back to the
plain token scan.

#### Scenario: A module member resolves to the bound file

- **WHEN** a `.tsx` source has `import s from './card.module.css'` then `s.btnPrimary`
- **AND** `card.module.css` defines `.btn-primary`
- **THEN** a `uses_css_class` edge resolves to that class with an `Exact` grade

### Requirement: CSS-internal dependency edges

These at-rules are resolved away by compilation, so they SHALL be extracted by a
**light source scan** (at-rule keyword spotting, not Sass parsing): `@import` /
`@use` / `@forward` as `imports` between the stylesheet **module** nodes; `@extend
.class` and `composes … from` as `extends_rule` targeting the referenced
`css_class`. Cross-file targets SHALL resolve against the building store with
keep-all on irreducible ambiguity; a target that resolves to nothing emits no
edge.

Because the scan is not a Sass parser, `extends_rule` SHALL be emitted only when
the **enclosing rule is a single bare class selector** (`.btn-primary { @extend
.btn }`). Enclosing selectors that are compound (`.a.b`), descendant (`.a .b`),
`&`-nested, interpolated (`.#{$prefix}x`), element/pseudo (`h1`, `&::after`), or a
mixin body SHALL yield no edge rather than a wrong one — consistent with the
no-dangling-stubs rule.

`@extend %placeholder` and `@include`/mixin are **out of scope** (not merely
deferred for a node kind). Real-world Sass overwhelmingly extends placeholders
from exactly the unresolvable enclosings above — interpolated class selectors
(Bulma extends `%x` from `.#{$prefix}name` throughout) and element selectors
(Bootstrap extends `%heading` from `h1`–`h6`) — so a source scan would index the
placeholder nodes yet emit ~no edges. Since `@extend` is compiled away (its styles
merge into selector groups), the compiled output cannot recover the relationship
either. Placeholder/mixin support is therefore parked until a concrete codebase
that uses sole-bare-class `@extend %x` justifies it.

#### Scenario: An @use becomes a module imports edge

- **WHEN** a stylesheet contains `@use './tokens';`
- **THEN** an `imports` edge is emitted to the `tokens` stylesheet module node

#### Scenario: A cross-file composes resolves to a class

- **WHEN** `composes: base from './shared.module.css'` references `.base`
- **THEN** an `extends_rule` edge resolves to that `css_class`

#### Scenario: An unresolvable enclosing selector yields no extends edge

- **WHEN** an `@extend` sits in an interpolated, element, compound, `&`-nested, or
  mixin rule (e.g. `.#{$prefix}file { @extend %block }` or `h1 { @extend %heading }`)
- **THEN** no `extends_rule` edge is emitted (the enclosing is not a sole bare class)

### Requirement: Code-to-stylesheet import edges

The indexer SHALL recover stylesheet imports that code indexers drop (a
`.css`/`.scss`/`.sass` specifier has no code declaration, so the importing edge
is never emitted). Scanning `usage_sources` files at the post-code barrier, it
SHALL identify stylesheet import specifiers **by MIME type** (`text/css`,
`text/x-scss`, `text/x-sass`), resolve each relative specifier to a stylesheet
`module` node in the building store, and emit an `imports` edge from the
**importing code module** to that stylesheet module. The edge reuses
`EdgeKind::Imports` (so a stylesheet module's inbound `imports` may mix
stylesheet importers and code importers, distinguished by source language). A
specifier that resolves to no indexed stylesheet emits no edge (no dangling
stubs); non-relative/aliased specifiers (`@/…`, `~pkg/…`) are out of scope for v1
because path-mapping lives in the code indexer, which cannot carry a
cross-producer edge.

#### Scenario: A CSS import becomes a cross-language imports edge

- **WHEN** a `.ts` file under a `usage_sources` glob has `import './button.css'`
- **AND** `button.css` is indexed as a stylesheet module
- **THEN** an `imports` edge is emitted from the `app.ts` module to the
  `button.css` module

#### Scenario: A non-stylesheet import is ignored

- **WHEN** the import specifier is `./data.json` (MIME not a stylesheet type)
- **THEN** no cross-language `imports` edge is produced

### Requirement: Code-sourced edges wait for the post-code barrier; CSS-internal does not

Code-sourced edges (`uses_css_class` and code→stylesheet `imports`) SHALL be
gated on the post-code barrier — because their **source endpoint is a code node**
(enclosing symbol, code file, or code module), which exists only once code ingest
has populated the store (the same barrier markdown→code resolution uses).
Candidate *extraction* reads source text from disk and needs no store; only edge
**emission** needs the barrier. **CSS-internal resolution**
(`imports`/`extends_rule`, stylesheet↔stylesheet) SHALL NOT wait on the code
barrier — both its endpoints are stylesheet nodes, so it resolves once the
stylesheet producer is done and MAY overlap code ingest. A run with only
stylesheets (no code) SHALL still publish the full stylesheet corpus including
CSS-internal edges, with `uses_css_class` simply having no code sources to attach to.

#### Scenario: Usage resolves after code ingest, CSS-internal need not

- **WHEN** code ingest units are still running but the stylesheet producer is done
- **THEN** `imports`/`extends_rule` edges MAY already be resolved
- **AND** `uses_css_class` edges are not emitted until code ingest completes

#### Scenario: A stylesheet-only run still publishes

- **WHEN** a run has stylesheets but no code
- **THEN** the stylesheet corpus (nodes + CSS-internal edges) publishes
- **AND** no `uses_css_class` edges are emitted (no code sources exist)

### Requirement: Orphan and dangling hygiene report (`check_css`)

kenn SHALL expose a `check_css` report (cousin of `check_links`) that surfaces:
**orphan classes** (defined, zero inbound `uses_css_class`), **orphan stylesheets**
(no used selectors and no inbound `imports`), and — when a utility allowlist is
configured — **dangling code classes** (used, no definition, not a utility).
Output SHALL be bounded and filterable by category; each finding SHALL carry its
source location (the definition site for an orphan class/stylesheet, the usage
site for a dangling code class). Orphan-class detection SHALL be **gated on `usage_sources`
being configured**: when usage mining is inactive, every class trivially has zero
inbound `uses_css_class`, so `check_css` SHALL skip the orphan-class category and
state that it requires `usage_sources` — rather than report every class as
orphaned. Orphan-stylesheet detection (based on `imports`) is unaffected.

#### Scenario: An unused class is reported as an orphan

- **WHEN** `usage_sources` is configured AND a class has no inbound `uses_css_class`
- **THEN** `check_css` lists it as an orphan class with its definition site

#### Scenario: Orphan-class detection is skipped when usage mining is off

- **WHEN** `usage_sources` is unset
- **THEN** `check_css` does not report orphan classes
- **AND** it states that orphan-class detection requires `usage_sources`

#### Scenario: A dead stylesheet is reported

- **WHEN** a stylesheet has no used selectors and is not imported by any other
- **THEN** `check_css` lists it as an orphan stylesheet

#### Scenario: Dangling code classes require the allowlist

- **WHEN** no utility allowlist is configured
- **THEN** `check_css` does not report dangling code classes (to avoid
  Tailwind/3rd-party false positives)
