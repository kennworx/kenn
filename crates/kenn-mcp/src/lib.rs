//! `kenn-mcp` — MCP read-server for kenn snapshots.
//!
//! The transport half of kenn's read path. It owns the JSON-RPC surface, the
//! server lifecycle (indexing → embedding → ready), the file watcher, and the
//! reindex orchestration; the questions themselves live in [`kenn_query`],
//! which this crate wraps and the CLI calls directly.
//!
//! Concretely, a `#[tool]` wrapper here does three things and no more: gate the
//! snapshot, build a [`kenn_query::QueryCtx`], and render the result — see
//! `server/core.rs`. Surface, envelopes, cursor format, and error model are
//! defined by the `mcp-server` and `findings-mcp` `OpenSpec` proposals; the
//! mapping from a [`kenn_query::QueryError`] onto JSON-RPC's numeric space is
//! this crate's business alone (`server/errors.rs`).

pub mod index_status;
pub mod indexing;
pub mod server;
pub mod state;
pub mod tools;
pub mod watcher;

pub use index_status::{IndexStatus, IndexStatusProgress};
pub use state::{WorkspaceSource, WORKSPACE_DISCOVERY_TARGET};
