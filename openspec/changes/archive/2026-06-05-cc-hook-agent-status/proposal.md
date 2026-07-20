## Why

The collector records *what* an agent did (commands, files) and *where* it lives
(tmux), but not *what it is doing right now* — working, idle, waiting for input —
nor how many subagents it has in flight. A consumer (e.g. a multi-session
dashboard) wants each session's live status and active-subagent count.

Claude Code emits no explicit "status" event; status must be **inferred** from
the lifecycle hooks, and the needed signals (`Stop`, `SubagentStop`,
`Notification`, `PreToolUse(Task)`) are not wired yet.

## What Changes

- **Track main-agent status** (coarse — no per-tool firing):

  | hook | status |
  |---|---|
  | `UserPromptSubmit` | `working` |
  | `Notification` | `needs_permission` if the message names a permission, else `needs_input` |
  | `Stop` | `idle` |

  The raw `Notification` message is stored as `detail`. Status is best-effort and
  Claude-Code-version-sensitive (message strings); the raw detail is preserved.

- **Count active subagents instead of guessing "waiting."** Spawning a subagent
  is **not** the same as waiting for one (a background subagent runs while the
  main agent keeps working). So rather than a `waiting_for_subagent` status, the
  session carries an `active_subagents` count: `PreToolUse(Task)` increments it,
  `SubagentStop` decrements it (clamped at 0). The count is correct for
  foreground, background, and parallel subagents; the consumer interprets it.

- **Store current status + history.** `status` + `status_at` (and
  `active_subagents`) on the `sessions` row hold the live state; an append-only
  `session_status(session_id, status, detail, t)` table is the status timeline.

- **Wire the new hooks.** `cc-hook install` + `hooks.json` add `Stop`,
  `SubagentStop`, `Notification`, and `PreToolUse(Task)`. New `kenn cc-hook`
  subcommands: `stop`, `subagent-stop`, `notification`, `pretool-task`. The
  existing `prompt` handler also stamps `working`.

## Capabilities

### Modified Capabilities

- `conversation-history-store`: adds agent-status tracking — `status`/`status_at`
  + `active_subagents` on `sessions`, a `session_status` transition log, and the
  `Stop` / `SubagentStop` / `Notification` / `PreToolUse(Task)` hooks that drive
  them.

## Impact

- **Schema:** `status`, `status_at`, `active_subagents` on `sessions`; new
  `session_status` table (rewrite in place, no migration).
- **Hot path:** unchanged for the per-Bash path. New collector invocations only
  on `Stop` / `SubagentStop` / `Notification` / `PreToolUse(Task)` — all
  infrequent, none per-tool.
- **Payload:** `HookInput` gains `message` (the `Notification` text).
- **Inference caveat:** status is derived, not authoritative; the
  idle-vs-needs-input distinction depends on whether a `Notification` fires.
