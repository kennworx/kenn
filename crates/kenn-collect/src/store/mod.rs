//! SQLite-backed event store for the agent file-write collector.
//!
//! A single global `collector.db` under `kenn_server::paths::state_dir()`
//! (design §D2), written directly by each short-lived `kenn cc-hook` process
//! (§D1). WAL journaling + `busy_timeout` let concurrent writers across
//! sessions retry rather than fail with `SQLITE_BUSY`.
//!
//! Schema: `sessions → commands → files` (§D5). One `insert_file` serves both
//! Edit/Write touches (`command_id` NULL) and Bash outputs (`command_id` set).

mod core;
pub use core::*;
