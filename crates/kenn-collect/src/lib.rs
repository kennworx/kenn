//! Agent file-write collector.
//!
//! Ports periClaude's bash-AST file-capture mechanism: a Bash command parser
//! (`parser`) built on `brush-parser` and a global, project-keyed `SQLite` store
//! (`store`, `schema`, `gc`) written directly by each `kenn cc-hook` process.
//!
//! Design and rationale live in
//! `openspec/changes/track-agent-file-writes/design.md` (§D1–D10).

pub mod gc;
pub mod parser;
pub mod schema;
pub mod store;

pub use parser::{parse, ParseError, ParsedCommand};
pub use store::{
    collector_state_dir, AgentStatus, FileChannel, Output, OutputKind, SessionMeta, Store,
};
