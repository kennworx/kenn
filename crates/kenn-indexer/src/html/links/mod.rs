//! HTML link, import & asset glue (design D1 Tiers 1–2, Phases 1–2).
//!
//! Connective edges hang off the document node the keystone built:
//!
//! - `<a href>` → a link edge, **reusing the markdown file/path resolver**
//!   ([`resolve_file_ref`], the file branch of the md→code ladder). The
//!   `<a href>` ladder (design D7, by what the target resolves to):
//!   - a fragment (`#frag` / `page#frag`) → `LinksTo` the target file's
//!     `html_id` anchor (Phase 2 — the HTML analog of markdown sections); an
//!     unknown fragment dangles.
//!   - an indexed file → `LinksToFile` (files table), carrying the resolver's
//!     Exact / Drifted / Ambiguous grade.
//!   - a non-indexed asset that exists on disk → `LinksTo` a path-keyed
//!     `attachment` stub (symbol space, Phase 2 — see [`asset_link_edges`]).
//!   - anything else → a `Dangling` `LinksTo` to an `html:@unresolved/…` stub
//!     (never dropped, mirroring markdown).
//! - `<link rel="stylesheet" href>` (HTML→CSS) and `<script src>` (HTML→JS)
//!   → `Imports` edges from the document to the referenced file (design D7:
//!   imports hydrate from the files table). Same file resolver; a target not in
//!   the workspace becomes a dangling stub rather than being dropped.
//! - `<img>`/`<video>`/`<source>`/`<iframe>` `src` → `LinksTo` an `attachment`
//!   stub keyed by canonical workspace-relative path (Phase 2,
//!   [`asset_link_edges`]), so every spelling of one asset collapses to one node.
//!
//! These are pure functions over a [`parse_elements`](super::parse) element list
//! plus caller-built lookups (the workspace file set, the per-file `html_id`
//! anchor index, the on-disk asset set) — the same isolation the markdown
//! resolver's unit tests use. Pipeline wiring is Phase 4.

mod core;
pub use core::*;
