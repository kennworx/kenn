//! Storage backend: `SQLite` (engine `sqlite`).
//!
//! The code graph + search store are `SQLite` databases (`code.db`,
//! `vector.db`) and the findings store is records + a transient `SQLite`
//! FTS index. Lance/DataFusion/Arrow are gone.
//!
//! Module map:
//! - `sqlite` — the `SQLite` backend: `DbReader` / `DbWriter`, schema, search.
//! - `codes` — backend-neutral edge codes + knowledge-text helpers.
//! - `findings` — the durable, records-based findings store.
//! - `jobs` — snapshot-level jobs (`reembed`, `embed_pending`,
//!   `stage_findings_for_publish`).

mod codes;
mod findings;
mod jobs;
mod names;
mod sqlite;

pub use findings::{
    finding_is_stale, Anchor, AnchorEvent, AnchorHealth, BrokenAnchors, CodeGraphNodeResolver,
    CodeNodeResolver, DriftedAnchors, FindingsStore, UnverifiedClaim,
};
pub use jobs::{
    embed_pending, read_embed_error, reembed, stage_findings_for_publish, ReembedReport,
};
pub use sqlite::{DbConn, DbReader, DbWriter};
