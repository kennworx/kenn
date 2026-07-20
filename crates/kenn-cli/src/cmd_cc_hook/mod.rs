//! `kenn cc-hook` — capture Claude Code hook events into the global collector
//! store.
//!
//! Each invocation reads ONE hook JSON payload from stdin and writes directly
//! to `<state_dir>/collector.db` (a single, global, project-keyed `SQLite` store
//! — design §D1, §D2). There is no daemon and no per-workspace JSONL.
//!
//! Subcommands:
//!   - `session-start` / `prompt` / `session-end` → session lifecycle rows.
//!   - `pretool-bash` (`PreToolUse` Bash) → a `commands` row (running) plus
//!     parsed redirect/tee `files` rows (§D3).
//!   - `posttool-bash` (`PostToolUse` Bash) → finish the matching command by
//!     `tool_use_id` (§D3).
//!   - `touch` (`PostToolUse` Edit|Write) → a path-only `files` row (§D7).
//!
//! `session-start` additionally emits a Claude Code `additionalContext` block
//! on **stdout** — standing instructions for the agent: pipe long commands
//! through `tee` (which also feeds our redirect/tee capture in §D3) and run
//! `/kenn:squeeze` before committing work that captured durable steering. The
//! reminder is advisory only — no tool call is ever blocked. Stdout is reserved
//! for that injected context; any error on the injection path is routed through
//! `tracing` (→ stderr) and never written to stdout.
//!
//! Graceful-failure invariant (§D8): every recoverable error (malformed JSON,
//! unwritable DB, parse failure, missing field) is appended to
//! `<state_dir>/cc-hook.log` and the subcommand exits 0. The hook must never
//! interrupt the user's interactive Claude Code session.

mod core;
pub use core::*;
