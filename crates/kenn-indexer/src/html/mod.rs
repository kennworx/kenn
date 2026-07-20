//! HTML corpus producer (design D1, Phase 0 keystone).
//!
//! HTML is indexed as a sibling producer to the SCIP path (like markdown and
//! css): discovered, parsed with html5ever's WHATWG tree builder, and emitted as
//! a `document` node per file. [`parse_elements`] is the load-bearing internal
//! API — a flat, line-tagged element list later tiers (links, imports, ids,
//! class usage, inline style) consume; this phase only builds and proves it.

mod classes;
mod discover;
mod ids;
mod ingest;
mod links;
mod parse;
mod styles;

pub use classes::{class_usage_edges, ClassRegistry};
pub use discover::{discover_html, DiscoveredHtml, HtmlDiscoverError};
pub use ids::{correspondence_edges, html_id_nodes, CssIdLookup, HtmlIdIndex, HtmlIdNodes};
pub use ingest::{
    ingest_html, resolve_html, HtmlCounts, HtmlIngestError, HtmlPending, HtmlResolveCounts,
};
pub use links::{
    anchor_link_edges, asset_link_edges, import_edges, AssetIndex, FragmentIndex, HtmlIds,
    StubSink, WorkspaceFiles,
};
pub use parse::{parse_elements, style_blocks, Attr, Element, StyleBlock};
pub use styles::{inline_style_nodes, InlineStyleNodes};
