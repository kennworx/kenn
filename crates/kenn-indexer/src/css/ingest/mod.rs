//! Stylesheet ingest: the sibling-producer pass (like markdown's phase 1).
//!
//! Discovers stylesheets and emits their nodes/edges through the `BatchSink`.
//! `.css` is parsed directly by lightningcss; `.scss`/`.sass` entry points are
//! compiled by dart-sass and the compiled CSS is parsed by the same path, with
//! each selector attributed back to its origin `.scss` via the source map. If no
//! dart-sass compiler is found, Sass is left unindexed with a log (never fails).

mod core;
pub use core::*;
