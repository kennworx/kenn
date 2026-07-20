# Tasks

Phased per proposal.md. Phase 1 is self-contained (searchable stylesheet corpus
+ registry); Phase 2 depends on Phase 1's registry + the post-code barrier;
Phase 3 depends on Phase 1's parse output and Phase 2's usage edges.

## Phase 1 — parse + class registry + definition sites

### 1. Data model & identity (`kenn-model`)

- [x] 1.1 Add two languages: `Language::Css` (`css` prefix, `extensions(["css"])`) and `Language::Sass` (`sass` prefix, `extensions(["scss","sass"])` — one language, two syntaxes); neither has project files. `FileRecord.language` reflects the source file honestly. → verify: prefix/db_name round-trip incl. `css` and `sass`; `.scss` and `.sass` both map to `Sass`.
- [x] 1.2 Add `Kind::CssClass`, `Kind::CssId`, `Kind::CssVar` — **shared** across both languages (a class is a class regardless of source). The stylesheet **file** reuses the existing `Kind::Module` (a stylesheet is a module of rules); no new kind for it. → verify: `db_name`/`from_db_name` round-trip for the three css kinds.
- [x] 1.3 Define the **typed** native-ID builder `<lang>:<relpath>#<type>:<name>` (`type` ∈ `class`/`id`/`var`; e.g. `css:button.css#class:btn`, `…#id:app`, `…#var:--brand`); the file node is `<lang>:<relpath>`. → verify: `.hero` and `#hero` in one file yield distinct IDs; same class name in `.css` vs `.scss` distinct by prefix.

### 2. Config & discovery (`kenn-config`, `kenn-indexer`)

- [x] 2.1 Add **one** `CssConfig` field on `LanguageConfig` covering css+sass: `roots` (stylesheet globs to parse) + `usage_sources` (globs to scan for class usage, Phase 2; **default empty** — explicit opt-in, since not all repo projects use CSS) + `excludes` (build-output denies `dist/`,`build/`,`node_modules/` are **always merged in**, not replaceable, so compiled CSS can't be silently re-admitted) + a nested `sass { compiler?, load_paths }` subsection. → verify: config parse test for roots/usage_sources/excludes + nested sass; `usage_sources` defaults empty; user excludes still keep the build-output denies.
- [x] 2.2 Stylesheet walker discovers `.css`/`.scss`/`.sass` across roots; compile globs to `GlobSet`; apply excludes; dispatch each file to its parser by extension. → verify: discovery over a fixture tree skips a `dist/` file.

### 3. `.css` parse via lightningcss (`kenn-indexer`)

- [x] 3.1 Add `lightningcss` dependency (pin). Parse with `error_recovery: true`. → verify: builds; smoke parse of a sample.
- [x] 3.2 Walk the stylesheet: emit the file `module` node + atomic class/id nodes (`Component::Class`/`ID`) + custom-property def nodes (`Property::Custom`), each `defined_in` the module (which `contains` them), each with file + byte-range def site. Resolve CSS-nesting `&` via parent tracking. → verify: extraction test over fixture (module owns atoms; atomic split of `.a.b`; var defs; `&` resolved).

### 4. `.scss`/`.sass` parse — dart-sass compiler required, no bespoke fallback (`kenn-indexer`)

- [x] 4.1 Compiler discovery: resolve `sass` from natural project locations in order — configured override → `node_modules/.bin/sass` → `node_modules/sass-embedded-<platform>/dart-sass/sass` + `node_modules/sass/...` → `PATH` → kenn-bundled binary (build recipe like `build-indexer-dotnet`). Use the CLI, **not** the `sass-embedded` Rust crate (protocol-stale). → verify: discovery picks the project's `node_modules` sass over a bundled one; `sass --version` resolves; clear log when none found.
- [x] 4.2 Entry-point discovery: non-`_`-prefixed `.scss`/`.sass` at configured roots; configurable `@use`/`@import` load paths. dart-sass handles both syntaxes — `.sass` needs no special path. → verify: entry discovery test incl. a `.sass` entry.
- [x] 4.3 Compile each entry with `--source-map`; extract atomic classes/ids/vars from compiled CSS via lightningcss (incl. `@each`/mixin-generated). → verify: Bulma+Bootstrap fixtures compile; generated class (e.g. `bg-primary-subtle`) present in registry.
- [x] 4.4 Back-map each compiled selector's location through the source map to the origin source:line:col for def sites. → verify: known class back-maps to expected partial+line (e.g. `.small` → `_variables.scss`).
- [x] 4.5 On compile failure: skip the entry, log the reason; one failure never sinks the run (no hand-rolled scanner). → verify: a deliberately-broken entry is skipped+logged while sibling entries still index.

### 5. Records & corpus (`kenn-indexer`)

- [x] 5.1 Emit nodes + `FileRecord(language=css|sass)` (matching the source) through `BatchSink` as a sibling producer (no SCIP/`transform`). → verify: css-only run publishes nodes, no SCIP docs.
- [x] 5.2 Feed selector text + adjacent comments into `*DocsRecord` so FTS5 + embeddings cover stylesheet nodes. → verify: `search_symbols`/`semantic_search` return css nodes.
- [x] 5.3 Build the **class registry** (atomic name → {node id, def site}) as a queryable post-Phase-1 artifact. → verify: registry lookup by name returns all defs; ambiguous names keep all.

## Phase 2 — usage mining + code↔class graph

### 6. Data model

- [x] 6.1 Add `EdgeKind::UsesCssClass` (code file/symbol → css class): the enum variant, `db_name`, the `EdgeProperties::UsesCssClass { grade: LinkGrade }` rich variant, the `kind()` mapping arm, and the writer flatten + storage column reuse for the grade (mirror markdown's `link_grade` plumbing). → verify: `edge_properties_kind_covers_every_variant` includes it; writer test asserts the grade persists.

### 7. Usage scan (`kenn-indexer`, post-code barrier)

- [x] 7.1 Tailwind-style candidate extractor over `usage_sources` files (raw text; quote/whitespace/backtick split). → verify: extractor unit test on JSX/HTML/TS snippets.
- [x] 7.2 Intersect candidates with the registry; emit a `uses_css_class` edge **only on a hit** (no edge/stub for misses), from the enclosing code symbol when resolvable else the code file node, graded by context (`Exact` in `class=`/`className=`/module-member context, else `Fuzzy`; `Ambiguous` when multiple registry defs share the name). → verify: hit emits edge with correct grade; miss emits nothing; `Exact` vs `Fuzzy` per context.
- [x] 7.3 CSS-Modules binding-aware path (JS/TS sources only — needs import-binding analysis, MAY reuse store import edges): `import s from './x.module.css'; s.foo` pinned to that file with camelCase↔kebab fold (`Exact`); non-JS sources fall back to the token scan. → verify: `.tsx` module resolution test (file binding + name fold); `.html` falls back.
- [x] 7.4 Collect undefined class-shaped tokens (registry miss, not a utility) for the `check_css` report (task 9.3), each retaining its **file + offset** (so the report can point at the usage site) — **not** stored as nodes/edges. → verify: undefined token surfaces in the report with its location; no graph node created.
- [x] 7.5 Run as a post-code-barrier step (mirror `resolve_markdown_code_unit`): the source endpoint is a code node, so edge emission waits for code ingest. → verify: usage edges emit only after code ingest; css-only run still publishes (no usage edges).
- [x] 7.6 When `usage_sources` is empty: skip usage mining, still publish Phase-1 corpus, emit a one-time hint that `usage_sources` is unset. → verify: empty config publishes stylesheets with no `uses_css_class` edges + hint logged.

## Phase 3 — CSS-internal graph + orphan/hygiene reports

### 8. CSS-internal edges

- [x] 8.1 Light source scan (at-rule keyword spotting — these at-rules are compiled away) → reuse `EdgeKind::Imports` for `@import`/`@use`/`@forward` between stylesheet **module** nodes; add `EdgeKind::ExtendsRule` for `@extend .class`/`composes` targeting `css_class` (variant + `db_name` + `EdgeProperties::ExtendsRule { grade }` + `kind()` arm + grade persistence + coverage test). `@extend %placeholder` and `@include`/mixin deferred (need placeholder/mixin kinds). → verify: edge-kind round-trip; `imports` connects module nodes; `@extend .class` resolves.
- [x] 8.2 Resolve `@extend .class` / `composes … from './x'` against the building store (locality/keep-all ladder). A target that resolves to nothing emits **no edge** (consistent with usage: no dangling stubs). → verify: cross-file composes resolves; missing target emits no edge.

### 9. Orphan / hygiene report (`check_css`)

- [x] 9.1 Orphan class: defined, 0 inbound `uses_css_class`. → verify: report flags an unused class, not a used one.
- [x] 9.2 Orphan stylesheet: no used selectors AND no inbound `imports`. → verify: report flags a dead sheet.
- [x] 9.3 Dangling code class (gated on utility allowlist): used, no def, not a utility. → **DEFERRED to its own change `css-dangling-report`** — unlike 9.1/9.2 (which report over existing graph data), dangling tokens are deliberately *not* persisted (an unmatched class emits no node/edge so Tailwind utilities don't inflate the graph). Reporting them needs a new persistence path for `UsageScan.undefined` + a `utility_allowlist` config field. Not blocking the rest of index-css.
- [x] 9.3b Gate orphan-class on `usage_sources` configured: when usage mining is off, skip the orphan-class category and state it requires `usage_sources` (don't flag every class). → verify: usage off → no orphan-class output + explanatory note; orphan-stylesheet still works.
- [x] 9.4 Expose `check_css` as an MCP tool mirroring `check_links` (bounded output + grade/category filter). → verify: tool returns categorized findings with written + resolved targets.

## Phase 4 — code→stylesheet imports (cross-language)

### 11. Code-to-stylesheet import edges (`kenn-indexer`, post-code barrier)

- [x] 11.1 Pure detection helpers in `css/usage.rs`: `extract_style_imports` (light line+keyword scan for quoted specifiers in `import`/`require`/`from`/`import(...)`/`export … from` context) + `is_stylesheet_import` (the MIME detector — `mime_guess::from_path` → `text/css`⇒`Css`, `text/x-scss`/`text/x-sass`⇒`Sass`). → verify: unit tests classify `.css`/`.scss`/`.sass` specifiers, reject `.ts`/`.json`/bare tokens.
- [x] 11.2 In `resolve_css_usage` (post-code barrier), for each `usage_sources` file scan stylesheet imports; resolve each relative specifier to a relpath (reuse `internal::normalize_join`), fetch the `css:`/`sass:` module node (`module_id` + `fetch_symbol`), resolve the importing file's module (`fetch_file_short_id` → inbound `contains`), and emit a module→module `imports` edge (`EdgeProperties::Imports{Explicit}`), deduped, skipping self-edges and unresolved targets. Add `import_edges` to `CssUsageCounts`. → verify: e2e — a `.ts` `import './button.css'` yields one inbound `imports` edge on the `button.css` module from the `app.ts` module; a `.json` import yields none.

## Cross-cutting

- [x] 10.1 Clippy clean (`cargo clippy --workspace --all-targets`), CRAP gate (`just crap-ci`), `cargo fmt --all` last.
- [x] 10.2 Update the kenn SKILL.md / agent-facing docs with the new node kinds, `uses_css_class`/`check_css`, and the `usage_sources` config (user-facing terms only).
