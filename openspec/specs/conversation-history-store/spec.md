# conversation-history-store Specification

## Purpose
Captures Claude Code session activity into a global, project-keyed SQLite store at `<state_dir>/collector.db`. The schema is relational — `sessions → commands → files` — recording which files a session touched: Bash command output destinations (discovered by parsing the command into an AST) plus `Edit`/`Write` touches. Capture happens entirely in short-lived `kenn cc-hook` processes with no embedding, LLM, or network calls.
## Requirements
### Requirement: hook events are captured via a `kenn cc-hook` CLI

The kenn CLI SHALL expose a `cc-hook <event>` subcommand that accepts Claude
Code hook JSON on stdin and records it into a global SQLite store (see "the
collected store is a global project-keyed SQLite database"). The subcommand
SHALL support events `session-start`, `prompt`, `pretool-bash`, `posttool-bash`,
`touch`, and `session-end`, mapped from the Claude Code hooks `SessionStart`,
`UserPromptSubmit`, `PreToolUse` (matcher `Bash`), `PostToolUse` (matcher
`Bash`), `PostToolUse` (matcher `Edit|Write`), and `SessionEnd` respectively.
The subcommand SHALL NOT perform any embedding, LLM call, or network call, and
SHALL NOT read, stat, or open any written file — its only side effects are the
SQLite write (and, on error, a diagnostic log append) plus, for `session-start`
alone, the stdout context injection described in "`session-start` injects a
standing tee instruction into Claude Code context".

#### Scenario: an Edit tool-use hook records a write file

- **WHEN** `kenn cc-hook touch` is invoked with `PostToolUse` JSON for an `Edit`
  of `src/foo.rs`
- **THEN** a `files` row is recorded with `channel: "edit"`, the file path, the
  session id, and a timestamp, and no `command_id`
- **AND** no `old_string` / `new_string` body is stored
- **AND** the subcommand exits 0

#### Scenario: a Read tool-use hook is not captured

- **WHEN** Claude Code performs a `Read`
- **THEN** no `cc-hook touch` is invoked for it (the install matcher is
  `Edit|Write`)
- **AND** no `files` row is recorded for the read

#### Scenario: a user-prompt hook records the prompt

- **WHEN** `kenn cc-hook prompt` is invoked with `UserPromptSubmit` JSON
- **THEN** the session's `last_prompt` is updated in the SQLite store

#### Scenario: a session-start hook records the session

- **WHEN** `kenn cc-hook session-start` is invoked with `SessionStart` JSON
- **THEN** a `sessions` row is upserted carrying `project`, `cwd`, `started_at`,
  and `last_seen_at`

### Requirement: `session-start` injects a standing tee instruction into Claude Code context

In addition to the SQLite capture, `kenn cc-hook session-start` SHALL emit a
Claude Code `additionalContext` block on stdout carrying a standing instruction
to redirect long-running shell output through `tee` into `./tmp/`. The block
SHALL be a single JSON object of shape `{"hookSpecificOutput": {"hookEventName":
"SessionStart", "additionalContext": <text>}}`, so Claude Code injects the
instruction into the agent's context at session start. The instruction text
SHALL be a committed markdown asset, not an inline string literal.

Injection SHALL be skipped when the `SessionStart` `source` is `resume` (the
instruction is already present in the replayed history) and SHALL occur for
`startup`, `clear`, and `compact`. Injection SHALL run before the capture write,
so the instruction is emitted even if the capture write fails. Stdout SHALL carry
only this JSON object; any serialization or write error SHALL be routed through
kenn's tracing channel (stderr), never stdout, and SHALL NOT change the exit
status (the graceful-failure contract holds — see "the CLI fails silently on
recoverable errors").

#### Scenario: session-start injects the tee instruction

- **WHEN** `kenn cc-hook session-start` is invoked with `SessionStart` JSON whose
  `source` is `startup`
- **THEN** stdout carries a single `hookSpecificOutput` JSON object with
  `hookEventName: "SessionStart"` and an `additionalContext` string naming `tee`
- **AND** the `sessions` row is still upserted

#### Scenario: a resumed session is not re-injected

- **WHEN** `kenn cc-hook session-start` is invoked with `source: "resume"`
- **THEN** stdout is empty (no re-injection)
- **AND** the `sessions` row is still upserted

### Requirement: Bash commands and their output files are captured by AST parse

`kenn cc-hook pretool-bash` SHALL parse the Bash command from the `PreToolUse`
payload into an AST and extract every output destination — `>`, `>>`, `&>`
redirects and `tee` targets — recording one `files` row per destination linked
to the command's `commands` row with `channel` of `redirect` or `tee`. Variable
expansion SHALL draw from both in-command assignments and the hook process's
ambient environment (`std::env`); a fully-resolved target SHALL be absolutized
against `CLAUDE_PROJECT_DIR` / cwd and stored with `resolved = true`, while an
unresolvable target SHALL be stored with `resolved = false` and its literal
text. The parse SHALL run in the hook process (not a server or daemon).

#### Scenario: a tee log is recorded the instant the command starts

- **WHEN** `kenn cc-hook pretool-bash` is invoked with `cargo test 2>&1 | tee
  ./tmp/test.log`
- **THEN** a `commands` row is recorded with `started_at` set and `finished_at`
  NULL
- **AND** a `files` row with `channel: "tee"`, `resolved: true`, and the
  absolutized path of `./tmp/test.log` is linked to it
- **BEFORE** the command finishes

#### Scenario: an ambient environment variable resolves a redirect target

- **GIVEN** `OUTDIR` is set in the hook process's environment
- **WHEN** `kenn cc-hook pretool-bash` is invoked with `build > $OUTDIR/run.log`
- **THEN** the recorded `files` row has `resolved: true` with `$OUTDIR` expanded

#### Scenario: an unresolvable redirect target is recorded literally

- **WHEN** `kenn cc-hook pretool-bash` is invoked with `build > $UNKNOWN/run.log`
  where `UNKNOWN` is unset
- **THEN** a `files` row is recorded with `resolved: false` and the literal
  target text

### Requirement: a Bash command's running state is tracked across Pre and Post

A `commands` row inserted by `pretool-bash` SHALL have `started_at` set and
`finished_at` NULL. `kenn cc-hook posttool-bash` SHALL locate that row by
`tool_use_id` and set its `finished_at` (and exit code when present in the
payload). A row with a fresh `started_at` and a NULL `finished_at` therefore
denotes a still-running command, so a consumer can find the live logs of a
long-running task.

#### Scenario: Post finishes the command Pre started

- **GIVEN** `pretool-bash` recorded a command with `tool_use_id = "t1"` and NULL
  `finished_at`
- **WHEN** `kenn cc-hook posttool-bash` is invoked with `tool_use_id = "t1"`
- **THEN** that command row's `finished_at` is set
- **AND** its output `files` rows are unchanged

### Requirement: the collected store is a global project-keyed SQLite database

The capture sink SHALL be a single SQLite database at `<state_dir>/collector.db`
(the OS state directory resolved for the kenn daemon), opened in WAL mode with a
`busy_timeout` so concurrent short-lived hook processes across sessions and
workspaces can write without `SQLITE_BUSY` failures. The schema SHALL be
`sessions → commands → files`, where `files.command_id` is nullable (NULL for
`edit`/`write` touches, set for Bash outputs) and every row carries a `project`
column derived from `CLAUDE_PROJECT_DIR` (fallback git toplevel, then cwd). The
`commands` and `files` rows SHALL additionally carry a `branch` column recording
the git branch in effect when the event occurred, so history can be filtered by
project *and* branch — whole-project history (`WHERE project = ?`) and
current-branch history (`WHERE project = ? AND branch = ?`). Branch is captured
per event (not per session, since a session may switch branches) and is derived
without spawning git — by reading the repository's `HEAD` directly (the linked-
worktree `.git`-file pointer is followed); a non-git location or an unreadable
`HEAD` yields a NULL branch.

The `sessions` row SHALL additionally carry session metadata captured at
`SessionStart`: `source` (the start reason — startup / resume / clear / compact),
`transcript_path` (a pointer to the session's conversation JSONL), `os_user`
(the OS `$USER`), and the session's terminal location as `tmux_pane`
(`$TMUX_PANE`, e.g. `%5`) and `tmux_socket` (the socket-path field of `$TMUX`).
The tmux fields SHALL be read from the hook process's environment — no `tmux`
subprocess — and are NULL when the session is not running inside tmux. Because a
tmux pane id is unique across its tmux server, the stored pane id is a complete
target for `tmux switch-client` from any other window/session. These fields SHALL
be set at `SessionStart` and backfilled (not overwritten with NULL) if a row was
first created by another hook.

The store SHALL NOT contain file sizes, file-existence confirmation, or edit-body
text. The store SHALL self-bound via a periodic retention/GC pass.

#### Scenario: concurrent hooks write the same database without error

- **GIVEN** two Claude sessions in different workspaces firing hooks at once
- **WHEN** both `kenn cc-hook` processes write `collector.db` concurrently
- **THEN** both writes succeed (WAL + `busy_timeout`)
- **AND** each row carries its own session's `project`

#### Scenario: rows are keyed by project across repositories

- **GIVEN** hooks fired from two different repositories
- **WHEN** the `files` table is queried
- **THEN** rows from each repository carry a distinct `project` value

#### Scenario: rows are keyed by branch within a project

- **GIVEN** a command captured while `main` is checked out, then another captured
  after switching to a `feature` branch in the same repository
- **WHEN** the `commands` table is queried
- **THEN** the two rows carry distinct `branch` values (`main` and `feature`)
- **AND** both carry the same `project`

#### Scenario: a non-git working directory yields a NULL branch

- **WHEN** a hook fires from a directory with no git repository
- **THEN** the recorded row's `branch` is NULL
- **AND** capture otherwise succeeds (the missing branch is not an error)

#### Scenario: the session row records its tmux location and provenance

- **GIVEN** a Claude session running in tmux pane `%5` started with source
  `resume`
- **WHEN** its `SessionStart` hook fires
- **THEN** the `sessions` row carries `tmux_pane = "%5"`, the `tmux_socket` from
  `$TMUX`, `source = "resume"`, the `transcript_path`, and `os_user`
- **AND** the stored `tmux_pane` is sufficient to `tmux switch-client` to that
  window from another session

#### Scenario: session metadata is backfilled, not clobbered

- **GIVEN** a `sessions` row first created by a non-start hook (so its metadata
  columns are NULL)
- **WHEN** the session's `SessionStart` hook later fires
- **THEN** the metadata columns are filled
- **AND** a subsequent `SessionStart` does not overwrite a populated field with NULL

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

### Requirement: the CLI fails silently on recoverable errors

`kenn cc-hook` SHALL exit with status 0 on any recoverable error — malformed
input JSON, an unwritable or locked SQLite store, a bash-parse failure, or a
missing payload field. Any such error SHALL be appended to a kenn diagnostic log
file but SHALL NOT propagate as a non-zero exit code that could interrupt the
user's Claude Code session. A bash-parse failure SHALL still record the command
row with zero output files. Programming errors (panics) are out of scope of this
requirement.

#### Scenario: a malformed hook payload does not break the session

- **WHEN** `kenn cc-hook touch` is invoked with stdin that is not valid JSON
- **THEN** the subcommand exits 0
- **AND** no row is written
- **AND** a diagnostic is appended to the kenn log

#### Scenario: an unparseable bash command still records the command

- **WHEN** `kenn cc-hook pretool-bash` is invoked with a command the parser
  rejects
- **THEN** a `commands` row is recorded with zero associated output files
- **AND** the subcommand exits 0

### Requirement: the hook latency budget is documented and benchmarked

The `kenn cc-hook` invocation SHALL complete in ≤5ms p95 under warm cache,
inclusive of the bash AST parse and the SQLite write, measured by a benchmark
that is part of the change. If the measured p95 exceeds 10ms in realistic
conditions, the documented opt-in hook configuration SHALL set `async: true` so
the user's session is not blocked.

#### Scenario: the benchmark is run as part of acceptance

- **WHEN** the documented benchmark is executed
- **THEN** the p95 latency is recorded
- **AND** the recorded value is referenced in the change's validation notes

### Requirement: `kenn cc-hook install` produces the hook-config snippet

The kenn CLI SHALL expose `cc-hook install` that prints, to stdout, the exact
JSON snippet to add to `~/.claude/settings.json` to wire `SessionStart`,
`UserPromptSubmit`, `PreToolUse` (matcher `Bash`), `PreToolUse` (matcher `Task`),
`PostToolUse` (matcher `Bash`), `PostToolUse` (matcher `Edit|Write`),
`Notification`, `Stop`, `SubagentStop`, and `SessionEnd` to the corresponding
`kenn cc-hook` subsubcommands. The subcommand SHALL accept an
optional `--write` flag that merges the snippet into the user's
`~/.claude/settings.json` in place; without `--write` it SHALL only print and
not modify the settings file.

#### Scenario: install prints by default

- **WHEN** `kenn cc-hook install` is invoked without flags
- **THEN** the snippet is printed to stdout
- **AND** `~/.claude/settings.json` is not modified

#### Scenario: install --write modifies settings

- **WHEN** `kenn cc-hook install --write` is invoked
- **THEN** the snippet is merged into `~/.claude/settings.json`
- **AND** the resulting file remains valid JSON
- **AND** previously-configured hooks for other events are preserved
