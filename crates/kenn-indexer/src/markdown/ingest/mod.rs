//! Markdown ingest: the sibling-producer pass (design D1), split across the
//! post-code join barrier (design D4).
//!
//! [`ingest_markdown_phase1`] runs in parallel with code ingest:
//!
//! 1. **collect** every file → build the global [`ResolutionIndex`].
//! 2. **walk** every file → emit its nodes + build a `pub_id → ShortId` map.
//! 3. **resolve md↔md** per file → emit `links_to`/`embeds` edges. A link that
//!    fails md resolution either becomes a dangling external stub now (external
//!    vault) or is *deferred* (in-repo, where it may still reference code).
//!
//! [`resolve_markdown_code`] runs after all code ingest units finish: each
//! deferred (in-repo only — design D6) link is resolved against the code graph
//! into md→code edges; whatever still doesn't match becomes a dangling stub.

mod core;
pub use core::*;
