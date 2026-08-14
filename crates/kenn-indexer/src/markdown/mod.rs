//! Markdown corpus producer.
//!
//! Markdown is indexed as a sibling producer to the SCIP path (design D1): it
//! is discovered, parsed, and emitted as `kenn_model` records directly,
//! without entering the SCIP `transform` chain. This module owns discovery
//! (`discover`); body parsing, the heading tree, and link resolution land in
//! later submodules.

mod code_resolve;
mod collect;
mod discover;
mod index;
mod ingest;
mod links;
mod resolve;
mod walk;

pub use code_resolve::{
    is_code_path, resolve_code_link, resolve_file_ref, CodeCandidate, CodeLookup, CodeTarget,
    StoreCodeLookup,
};
pub use collect::{collect, CollectedFile, Frontmatter, HeadingSlug, RelatedLink};
pub use discover::{discover_markdown, DiscoveredMarkdown, MarkdownDiscoverError};
pub use index::{NodeRef, ResolutionIndex};
pub use ingest::{
    existing_target_kind, ingest_markdown_phase1, resolve_markdown_code, FsPaths, MarkdownCounts,
    MarkdownIngestError, MarkdownPending,
};
pub use links::{extract_links, LinkKind, RawLink};
pub use resolve::{resolve_link, LinkTarget};
pub use walk::{walk_markdown, MarkdownIds, MarkdownRecords};
