## 1. Schema + store

- [x] 1.1 `kenn-collect::schema`: add `status TEXT`, `status_at INTEGER`, `active_subagents INTEGER NOT NULL DEFAULT 0` to `sessions`; add a `session_status(id INTEGER PK, session_id TEXT, status TEXT NOT NULL, detail TEXT, t INTEGER NOT NULL)` table + index on `(session_id, t)` (no migration, design D5).
- [x] 1.2 `kenn-collect::store`: add an `AgentStatus` enum (`Working`, `Idle`, `NeedsInput`, `NeedsPermission`) with `as_str`; a `set_status(session_id, AgentStatus, detail, now)` op that inserts a `session_status` row AND updates `sessions.status` / `status_at` (design D3).
- [x] 1.3 `kenn-collect::store`: add `bump_subagents(session_id, delta, now)` updating `sessions.active_subagents = MAX(0, active_subagents + delta)` and `last_seen_at` (design D2). Re-export `AgentStatus`.

## 2. Hook wiring

- [x] 2.1 Add `message: Option<String>` to `HookInput`.
- [x] 2.2 New subcommands + handlers in `cmd_cc_hook.rs`: `stop` → `set_status(Idle)`; `subagent-stop` → `bump_subagents(-1)`; `notification` → classify `message` (`needs_permission` if it contains "permission", else `needs_input`) → `set_status(..., detail = message)`; `pretool-task` → `bump_subagents(+1)`. Each `upsert_session` first; graceful failure preserved (design D4).
- [x] 2.3 `handle_prompt` additionally calls `set_status(Working)`.

## 3. Install

- [x] 3.1 `cc-hook install` snippet + `claude-plugins/kenn/hooks/hooks.json`: add `Stop` → `stop`, `SubagentStop` → `subagent-stop`, `Notification` → `notification`, and `PreToolUse`(matcher `Task`) → `pretool-task`. Existing wiring kept; idempotent merge preserved.

## 4. Verification

- [x] 4.1 Tests: `set_status` writes the `session_status` row and updates the `sessions` current status; `prompt`→working / `stop`→idle; `notification` classifies permission vs input and stores the raw message; `bump_subagents` increments on spawn, decrements on stop, clamps at 0; spawn→stop returns the count to 0. (kenn-collect store tests + 3 cc_hook_smoke subprocess tests.)
- [x] 4.2 `cargo clippy --workspace --all-targets` zero warnings.
- [x] 4.3 `just crap-ci` green for touched functions. *(`set_status` CRAP 3.0, `bump_subagents` 2.0; the gate passed — an earlier OOM in an overloaded sandbox was transient.)*
- [x] 4.4 `cargo fmt --all` as the final step.
