## Context

The collector's `sessions` row holds id / project / cwd / timestamps /
`last_prompt` / metadata (source, tmux, …). This change adds the agent's live
*status* and *active-subagent count*, inferred from lifecycle hooks Claude Code
already emits but the collector doesn't yet wire.

## Decisions

### D1 — Status is inferred from hooks; four states

Claude Code has no "status" event. Status is derived:

- `UserPromptSubmit` → `working` (a turn started).
- `Stop` → `idle` (the turn ended).
- `Notification` → `needs_permission` if the message contains "permission",
  else `needs_input`. The raw message is stored as `detail` so the
  classification is best-effort but lossless.

`working` / `idle` / `needs_input` / `needs_permission` are the states. Status is
coarse — it is **not** driven by per-tool `PreToolUse`/`PostToolUse`, so the
collector keeps firing only on the lifecycle hooks, not on every tool call.

### D2 — Count active subagents; do NOT infer "waiting"

Spawning a subagent is not waiting for one: a background subagent runs while the
main agent keeps working, and `SubagentStop` fires only when it finally ends.
Tying a `waiting_for_subagent` status to spawn (or holding it until
`SubagentStop`) would therefore lie. Instead the session carries an integer
`active_subagents`:

- `PreToolUse(Task)` → `active_subagents = active_subagents + 1`.
- `SubagentStop` → `active_subagents = MAX(0, active_subagents - 1)`.

Each `Task` spawn yields exactly one eventual `SubagentStop`, so the count is the
number of in-flight subagents — correct for foreground, background, and parallel
fan-out alike. `MAX(0, …)` guards against a missed spawn hook. The main-agent
`status` is unaffected by subagents (it stays `working` while a turn is active);
the consumer decides what "working + N subagents" means.

### D3 — Current status on `sessions`, history in `session_status`

`sessions` gains `status TEXT`, `status_at INTEGER`, and
`active_subagents INTEGER NOT NULL DEFAULT 0` — the live view. An append-only
`session_status(id, session_id, status, detail, t)` table (indexed on
`(session_id, t)`) is the transition timeline: current status = the `sessions`
column, history = the table. A status write does both (one log insert + one
`sessions` update). Subagent count changes update only the `sessions` counter —
they are not status transitions.

### D4 — New subcommands; ensure-session first; graceful failure preserved

`handle_stop` / `handle_subagent_stop` / `handle_notification` /
`handle_pretool_task` each `upsert_session` (the row must exist) then apply their
effect, reusing the existing graceful-failure contract (any recoverable error →
exit 0 + diagnostic log). `handle_prompt` additionally stamps `working`.
`HookInput` gains `message` for the `Notification` text.

### D5 — No migration

The new columns + table are added to the schema directly; any pre-existing
`collector.db` is abandoned (prototype convention).

## Risks / Trade-offs

- **`idle` vs `needs_input` depends on a `Notification` firing.** After `Stop`
  the session is `idle`; it only becomes `needs_input`/`needs_permission` if
  Claude Code emits a `Notification`. This mirrors what the user actually sees.
- **Version sensitivity.** The `Task` tool name and `Notification` message
  strings are Claude Code's. Storing the raw message hedges classification drift;
  if the subagent tool were renamed, the count would miss — acceptable for a
  best-effort provenance log.
- **Missed hooks skew the count.** A dropped `PreToolUse(Task)` or `SubagentStop`
  (Claude crash, hook timeout) can leave `active_subagents` off; `MAX(0, …)` and
  the next `SessionStart`/`Stop` bound the damage. It is advisory, not exact.
