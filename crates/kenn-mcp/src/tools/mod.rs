//! The tools that need a running server.
//!
//! What is left here after the `kenn-query` split is exactly the set a second
//! front end could not implement: index status, reindex, and the file watcher
//! all describe or drive *this process*. Every other tool the MCP surface
//! exposes is a [`kenn_query`] function, wrapped in `server/core.rs`.

mod lifecycle;
mod state;

#[cfg(test)]
mod tests;

pub use lifecycle::{
    get_index_status, reindex, wait_for_index, watch_start, watch_stop, GetIndexStatusArgs,
    ReindexArgs, ReindexResponse, WaitForIndexArgs, WaitForIndexResponse, WatchStartArgs,
    WatchStopArgs, WatchStopResult,
};
pub use state::{ReadyView, ServerState, WatchStartResult};
