## ADDED Requirements

### Requirement: agent status and active-subagent count are tracked from lifecycle hooks

The collector SHALL track each session's coarse agent status, inferred from
lifecycle hooks (Claude Code emits no explicit status event). The status SHALL be
one of `working`, `idle`, `needs_input`, or `needs_permission`, derived as:
`UserPromptSubmit` → `working`; `Stop` → `idle`; `Notification` →
`needs_permission` when its message names a permission, otherwise `needs_input`.
The raw `Notification` message SHALL be stored as the transition's `detail`, so
the classification is best-effort but the original text is preserved. Status SHALL
be coarse — it SHALL NOT be driven by per-tool `PreToolUse`/`PostToolUse`, so the
collector does not fire on every tool call.

The session SHALL also carry an `active_subagents` integer count — the number of
subagents currently in flight — maintained as: `PreToolUse` (matcher `Task`)
increments it, `SubagentStop` decrements it, clamped at zero. Spawning a subagent
SHALL NOT be treated as the main agent "waiting": a background subagent runs while
the main agent keeps working, so the count (not a waiting status) is what is
recorded, and it is correct for foreground, background, and parallel subagents.

The current status SHALL be stored on the `sessions` row (`status`, `status_at`,
`active_subagents`) and every status transition SHALL be appended to a
`session_status` table (`session_id`, `status`, `detail`, `t`) — current status is
the `sessions` column, the timeline is the table. The status hooks SHALL preserve
the graceful-failure contract (any recoverable error → exit 0 + diagnostic log).

#### Scenario: a prompt then stop moves working → idle

- **WHEN** `kenn cc-hook prompt` fires, then later `kenn cc-hook stop`
- **THEN** the `sessions` row's `status` is `working` after the prompt and `idle`
  after the stop
- **AND** a `session_status` row was appended for each transition

#### Scenario: a notification classifies and preserves its message

- **WHEN** `kenn cc-hook notification` fires with a message naming a permission
- **THEN** the status is `needs_permission`
- **AND** the transition's `detail` holds the original notification message

#### Scenario: active-subagent count rises on spawn and falls on stop

- **GIVEN** a session with `active_subagents = 0`
- **WHEN** two `PreToolUse(Task)` spawns fire, then one `SubagentStop`
- **THEN** `active_subagents` is 1
- **AND** the main agent's `status` is unchanged by the subagent activity

#### Scenario: the subagent count never goes negative

- **WHEN** a `SubagentStop` fires with no matching recorded spawn
- **THEN** `active_subagents` stays at 0 (clamped), not negative

## MODIFIED Requirements

### Requirement: `kenn cc-hook install` produces the hook-config snippet

The kenn CLI SHALL expose `cc-hook install` that prints, to stdout, the exact
JSON snippet to add to `~/.claude/settings.json` to wire `SessionStart`,
`UserPromptSubmit`, `PreToolUse` (matcher `Bash`), `PreToolUse` (matcher `Task`),
`PostToolUse` (matcher `Bash`), `PostToolUse` (matcher `Edit|Write`),
`Notification`, `Stop`, `SubagentStop`, and `SessionEnd` to the corresponding
`kenn cc-hook` subsubcommands. The subcommand SHALL accept an optional `--write`
flag that merges the snippet into the user's `~/.claude/settings.json` in place;
without `--write` it SHALL only print and not modify the settings file.

#### Scenario: install prints by default

- **WHEN** `kenn cc-hook install` is invoked without flags
- **THEN** the snippet is printed to stdout
- **AND** `~/.claude/settings.json` is not modified

#### Scenario: install --write modifies settings

- **WHEN** `kenn cc-hook install --write` is invoked
- **THEN** the snippet is merged into `~/.claude/settings.json`
- **AND** the resulting file remains valid JSON
