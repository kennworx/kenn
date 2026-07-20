//! `kenn-mcp` — MCP read-server for kenn snapshots.
//!
//! 24 tools over the `kenn-store` reader snapshot and the durable
//! findings store — code-graph reads, unified search, and the
//! findings knowledge layer. Surface, envelopes, cursor format, and
//! error model are defined by the `mcp-server` and `findings-mcp`
//! `OpenSpec` proposals.

pub mod cursor;
pub mod error;
pub mod indexing;
pub mod result_cache;
pub mod server;
pub mod state;
pub mod tools;
pub mod types;
pub mod watcher;

pub use cursor::{
    decode_cursor, encode_list_cursor, encode_topk_cursor, encode_usages_cursor,
    snapshot_id_from_timestamp, CacheId, DecodedCursor, SnapshotId,
};
pub use error::{McpError, McpErrorCode};
pub use state::{WorkspaceSource, WORKSPACE_DISCOVERY_TARGET};
pub use types::{
    FileRef, Filters, FindUsagesResponse, FindingView, IndexStatus, IndexStatusProgress,
    ListResponse, Pagination, RankedCodeHit, RankedFindingView, SemanticSearchResponse,
    SingleResponse, SourceView, StoreFindingResponse, SymbolDetail, SymbolRef, UsageRef,
    WorkspaceInfo,
};
