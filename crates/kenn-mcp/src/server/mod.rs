//! rmcp `ServerHandler` exposing the kenn-mcp tool surface over
//! stdio: code-graph query tools, the lifecycle `reindex` /
//! `watch_start` / `watch_stop` tools, and a `debug_env` diagnostic.
//! See the `#[tool_router]` impl block below for the canonical list.
//!
//! Each tool method is a thin wrapper around the corresponding async
//! function in [`crate::tools`] that:
//! 1. Translates the rmcp `Parameters<T>` wrapper into typed Args.
//! 2. `.await`s the tool function on the rmcp runtime.
//! 3. Maps `crate::error::McpError` into rmcp's `ErrorData`.
//!
//! Tool descriptions are kept ≤ 200 tokens each per design budget.

mod core;
mod env;
mod errors;
mod handler;

pub use core::*;
