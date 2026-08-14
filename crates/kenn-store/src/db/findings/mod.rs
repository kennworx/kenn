//! The durable findings store — agent-derived knowledge records.
//!
//! A finding is a durable, provenance-bearing statement about the
//! codebase (an invariant, a rationale, a gotcha). Each finding is
//! committed as a human-readable `.kenn/findings/<id>.md` record — the
//! source of truth. The search index and the finding embeddings are
//! *derived*: search is a transient in-memory FTS5 index built per-query
//! over the records, and embeddings ride a dedicated vector sidecar
//! (`.kenn/vectors/findings/`). Findings are append-only and durable.
//!
//! Module map:
//! - `record` — the committed `<id>.md` record files (D1).
//! - `store` — [`FindingsStore`], the public store surface (records-based
//!   read + transient-FTS5 / sidecar-vector blended search).
//! - `embed` — the findings embed pass over the committed records.
//! - `build` — the findings-publish lock acquired around the live flip.
//! - `lifecycle` — [`CodeNodeResolver`] + the supersede / tombstone /
//!   staleness primitives.
//! - `graph_resolver` — [`CodeGraphNodeResolver`], the read-time
//!   staleness probe (a snapshot's code-node id set).

mod anchor;
mod build;
mod directives;
mod embed;
mod graph_resolver;
mod index;
mod lifecycle;
mod record;
mod store;

pub use anchor::{Anchor, AnchorEvent, Outcome};
pub use build::acquire_findings_publish_lock;
pub use directives::{AnchorHealth, BrokenAnchors, DriftedAnchors, UnverifiedClaim};
pub(crate) use embed::embed_findings;
pub use graph_resolver::CodeGraphNodeResolver;
pub use lifecycle::{finding_is_stale, CodeNodeResolver};
pub use store::FindingsStore;
