//! CSS/Sass corpus producer.
//!
//! Stylesheets are indexed as a sibling producer to the SCIP path (like
//! markdown, design D1): discovered, parsed, and emitted as `kenn_model`
//! records directly, without entering the SCIP `transform` chain. `.css` is
//! parsed by lightningcss; `.scss`/`.sass` are compiled by dart-sass first (a
//! later step) and their output parsed by the same path.

mod discover;
mod extends;
mod ingest;
mod internal;
mod parse;
mod sass;
mod usage;

pub use discover::{discover_stylesheets, CssDiscoverError, DiscoveredStylesheet};
pub use ingest::{
    ingest_css_phase1, resolve_css_usage, CssCounts, CssIngestError, CssPending, CssUsageCounts,
};
pub use parse::{parse_css, CssIds, CssRecords};
pub use sass::{discover_sass_compiler, is_sass_entry};

// The bare CSS extractor, reused by the HTML producer for inline `<style>`
// blocks (design D6): the same selector collection + node/def/doc builders,
// emitted under the HTML file's `html:` owner. Crate-internal, not public API.
pub(crate) use parse::{collect_atoms, def, kind_of, preceding_comment, selector_text, symbol};
