# Conversation capture

`kenn cc-hook` captures Claude Code session activity — prompts, shell
commands, file edits, and agent status — into a single global collector
store. Each Claude Code lifecycle and tool event is wired to a
`kenn cc-hook <subcommand>` invocation that reads one hook JSON payload
from stdin and writes directly to the store.

There is **no daemon and no per-workspace file**: every invocation opens
one global, project-keyed SQLite database and writes to it.

## What gets captured

Ten subcommands across eight Claude Code hook events, writing into the
`sessions → commands → files` tables (plus `session_status`):

| Hook event | Matcher | Subcommand | Effect |
|---|---|---|---|
| `SessionStart` | — | `session-start` | Upsert the session row + metadata (source, transcript path, OS user, tmux pane/socket); inject the `tee` instruction (see below); trigger lazy GC |
| `UserPromptSubmit` | — | `prompt` | Store `last_prompt`; set agent status → `working` |
| `PreToolUse` | `Bash` | `pretool-bash` | Insert a *running* command row + parsed redirect/`tee` output files |
| `PreToolUse` | `Task` | `pretool-task` | Increment the session's active-subagent count |
| `PostToolUse` | `Bash` | `posttool-bash` | Finish the matching command (by `tool_use_id`) with its exit code |
| `PostToolUse` | `Edit\|Write` | `touch` | Insert a path-only file row |
| `SessionEnd` | — | `session-end` | Stamp `ended_at` |
| `Notification` | — | `notification` | Classify → `needs_permission` / `needs_input` |
| `Stop` | — | `stop` | Turn ended: agent status → `idle` |
| `SubagentStop` | — | `subagent-stop` | Decrement the active-subagent count |

The active-subagent count is balanced by `PreToolUse:Task` (+1) ↔
`SubagentStop` (−1): Claude Code has no "subagent start" event, so the
`Task` tool-use *is* the spawn signal.

Every row carries `project` (derived from the event's cwd: git toplevel,
else the cwd itself), so one database holds activity across all your
repositories. `commands` and `files` additionally carry the event-time
git `branch`.

## Where it is stored

A single SQLite file, `collector.db`, under the kenn **state directory**:

- `$KENN_STATE_DIR` (test/override), else
- `$XDG_STATE_HOME/kenn/`, else
- `~/.local/state/kenn/` (Linux **and** macOS — it does **not** use
  `~/Library/Application Support/`).

The diagnostic log (`cc-hook.log`) lives in the same directory. Tables:
`sessions`, `session_status`, `commands`, `files`, `meta` (schema in
`crates/kenn-collect/src/schema.rs`).

The collector never reads the filesystem — it records only what the hook
payloads contain (no file sizes, no content hashes).

## Context injection

`session-start` is the one subcommand that also writes to **stdout**: it
emits a Claude Code `additionalContext` block carrying a standing
instruction to pipe long-running command output through `tee` into
`./tmp/` (text in `crates/kenn-cli/src/session_start.md`). This both keeps
runs tailable and feeds the `tee`/redirect capture above — a command like
`cargo test 2>&1 | tee ./tmp/test.log` lands as a `files` row.

Injection is skipped on `SessionStart` `source: resume` (the instruction
is already in the replayed history). Stdout carries only that clean JSON;
any emission error is routed to `tracing` (stderr), never stdout.

## What is NOT captured

- File mutations inside a `Bash` command that aren't redirects/`tee`
  (e.g. `sed -i`, an editor invocation). The hook sees the command text
  and parses output redirections, not arbitrary side effects.
- `Read` tool use: the `PostToolUse` matcher is narrowed to `Edit|Write`.
- Anything before the hooks are wired into Claude Code.

## Trust boundary

Prompt text is stored **verbatim**. Anything you type or paste into a
Claude session — API keys, customer data, internal URLs — lands in
`collector.db` in clear, alongside every shell command you run.

Unlike a repo-local store, `collector.db` lives in your user state
directory, **outside any git tree**, and aggregates activity from every
project on the machine. It is readable by anything that can read your
home directory. kenn does not transmit captured data anywhere.

## Retention

30-day retention, enforced by lazy GC: `session-start` opportunistically
prunes sessions, commands, and files older than 30 days (running commands
and their files are never pruned). GC runs at most once per day, gated by
`meta.last_gc_at`. See `crates/kenn-collect/src/gc.rs`.

## Opt in

**If you use the `kenn` Claude Code plugin (recommended)** — the hooks
ship with the plugin (`claude-plugins/kenn/hooks/hooks.json`). Install or
reload the plugin and capture starts on the next Claude Code session; no
settings changes needed.

**If you run `kenn` as a standalone CLI** (no plugin) — use the install
helper to wire the hooks into your user-level settings:

```sh
# Print the snippet to inspect before installing:
kenn cc-hook install

# Merge it into ~/.claude/settings.json (idempotent; preserves any
# pre-existing hooks for other events):
kenn cc-hook install --write
```

**Do not do both.** Plugin hooks and settings hooks both fire if both are
present, producing duplicate records.

## Graceful failure

The hook sits in the user's interactive loop, so it must be cheap and
must never interrupt the session. Every recoverable error (malformed
JSON, unwritable DB, parse failure, missing field) is appended to
`<state_dir>/cc-hook.log` and the subcommand **exits 0**. Capture latency
is covered by `crates/kenn-collect/tests/latency.rs`.

## Inspecting captured data

`collector.db` is plain SQLite:

```sh
DB=~/.local/state/kenn/collector.db

# Most recent sessions:
sqlite3 "$DB" "SELECT id, project, status, last_prompt FROM sessions
               ORDER BY last_seen_at DESC LIMIT 10;"

# Every prompt in a project:
sqlite3 "$DB" "SELECT s.last_prompt FROM sessions s
               WHERE s.project LIKE '%my-repo%';"

# Files written in a session (Edit/Write touches + Bash tee/redirects):
sqlite3 "$DB" "SELECT path, channel FROM files WHERE session_id = '<id>';"

# Still-running commands:
sqlite3 "$DB" "SELECT cmd_text, started_at FROM commands
               WHERE finished_at IS NULL;"

# Recoverable errors (the hook always exits 0, but logs here):
tail -f ~/.local/state/kenn/cc-hook.log
```
