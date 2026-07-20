## Why

The atlas decomposes a repo by **anchor** (a package / crate / module) and forms
**domains only across anchors**. That assumes a multi-package repo — where kenn's
own atlas shines (many crates → many packages → real cross-crate domains). It
degrades badly on a **monolithic single-package project**, which is the common
case for a library: a Swift package is usually one module, a Rust library one
crate, a JS library one npm package.

Concrete evidence — indexing **Alamofire** (a large, real Swift library):

- **3 packages, only one substantive.** The anchors are the SwiftPM modules:
  `Alamofire` (the library), `AlamofireTests`, and the bundled `iOS_Example` app.
  The library's ~130 types all land in **one flat `Alamofire` concept** — no
  `Core` / `Features` / `Extensions` structure, even though the source is clearly
  organized that way. The producer never subdivides *within* an anchor.
- **1 domain, and it's spurious.** `build_domains` keeps only communities that
  span >1 anchor ("a single-package community just duplicates its package
  concept"). With one real library anchor, the *only* cross-anchor community is
  where the bundled example app references the library's `URLEncoding` — so the
  atlas invents a meaningless `URLEncoding` "domain" and nothing else.

The result is not a useful map of a big library: one giant flat package plus a
test/example package plus a nonsense domain. For any single-dominant-package
project the atlas has almost nothing to say.

## What Changes

Add **intra-package decomposition** so a dominant / monolithic package is mapped
by its own internal structure, not just its outer boundary:

1. **Source-directory sub-areas.** When one anchor holds many symbols across
   distinct top-level source subdirectories (e.g. `Source/Core`,
   `Source/Features`, `Source/Extensions`), emit a concept per subdirectory,
   parented to the package. A small or flat package is left as a single concept
   (no change).
2. **Intra-package domains.** When the repo is dominated by one anchor (few
   anchors overall), form domains from communities *within* the anchor — the
   semantic clusters the cross-anchor rule can't see — instead of returning
   nothing or a cross-anchor artifact.
3. **Example / sample suppression.** Bundled example / sample / demo code is
   excluded from domain eligibility (like tests already are), so a demo app that
   references a library type never fabricates a domain.

Multi-package repos (kenn itself, a C# solution) keep their current behavior; the
new decomposition only engages when a package is large and dominant.

## Capabilities

### New Capabilities
- `atlas-intra-package`: intra-package decomposition of a monolithic / dominant
  package into source-directory sub-areas and intra-package domains, so the atlas
  is useful for single-package libraries, not only multi-package repos.

### Modified Capabilities
<!-- The atlas capability's promoted spec is not yet in openspec/specs/ (the
     `atlas` change is still in progress), so this change adds its requirements as
     a new capability rather than a delta against an unpromoted spec. -->

## Impact

- `crates/kenn-indexer/src/atlas/producer.rs` — the decomposition + domain logic.
- `crates/kenn-indexer/src/atlas/{model.rs,okf.rs}` — a concept type / parent
  relationship for source sub-areas, and its OKF rendering.
- Atlas output shape for single-package repos changes (a reindex-generation
  concern; determinism preserved). Multi-package output is unchanged.
- No indexer / sidecar changes: this is pure aggregation-layer, data-dependent
  (reads the same aggregate + analysis tables).
