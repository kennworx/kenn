---
id: fnd_3755fffb-1535-436c-b09b-9fc1dabbc6f2
tags:
- directive
- polarity:dont
- supersedes:fnd_4c97e265-66c5-4be5-bc8a-ec4e06817f28
parent_ids:
- fnd_4c97e265-66c5-4be5-bc8a-ec4e06817f28
created_at: 2026-06-18T12:22:04.525935Z
---
Do NOT hand-roll a Sass parser or fallback scanner for `.scss`/`.sass`. A spike showed every off-the-shelf option is wrong/incomplete: tree-sitter-scss 1.0 lacks `!default`/`@use as` (21k+ ERROR nodes on Bootstrap, errors corrupt nearby nesting); grass lags the Dart-Sass stdlib (fails Bulma's `color.channel`); lightningcss has no Sass support. The implemented path (`crates/kenn-indexer/src/css/sass.rs`): `.scss`/`.sass` are compiled by the dart-sass compiler — discovered in order override→`node_modules/.bin/sass`→`sass-embedded-*` pkg→PATH→bundled, via the stable CLI (NOT the `sass-embedded` Rust crate, protocol-stale/panics) — then the compiled CSS is parsed by lightningcss (reusing `parse::collect_atoms`), capturing `@each`/mixin-GENERATED classes a source scan can't see. Only ENTRY points (`is_sass_entry`: non-`_`-prefixed) are compiled; partials reach the registry through an entry's compiled output, each selector attributed to its ORIGIN `.scss` via the source map (`SassExtract` dedupes origin files+selectors across entries). A failed compile is skipped+logged (`ingest_sass`), never partially scanned. A degraded mode, if ever needed, reuses lightningcss error-recovery over raw source — not new parser code.