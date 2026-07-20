## Why

The collector (`track-agent-file-writes`) keys every row by `project` (git
toplevel) but not by **branch**. An agent's work is branch-scoped — a long-lived
feature branch and `main` are different histories — and the collector can't tell
them apart, so a future reader can only ask "what happened in this repo?", never
"what happened on *this branch*?".

Separately, the collector (and the rest of kenn's runtime state) lands in
`~/Library/Application Support/kenn/` on macOS, because `state_dir()` falls back
to `data_local_dir()` there. The XDG-style `~/.local/state/kenn/` is wanted on
macOS too, for one predictable, greppable location across platforms.

## What Changes

- **Record the git branch per event.** Add a `branch` column to `commands` and
  `files`. Branch is captured at hook time (it can change within a session, so
  it belongs on the event row, not the session). It is derived **without a git
  subprocess** — by reading `<gitdir>/HEAD` directly (handling the linked-worktree
  `.git`-as-a-file case and detached HEAD) — so the ≤5ms hook budget is preserved.
  `project` is unchanged; the two compose: whole-project history is
  `WHERE project = ?`, current-branch history is `WHERE project = ? AND branch = ?`.
- **Move the state dir to `~/.local/state/kenn/` on macOS.** `state_dir()`
  resolves `$XDG_STATE_HOME` then `~/.local/state/kenn` on every platform,
  dropping the macOS `data_local_dir` branch. This moves `collector.db` +
  `cc-hook.log` **and** the daemon's `server.pid` / `server.log` (one shared
  state dir). The `kenn.toml` config path (a separate `config_dir`) is untouched.
- **No migration — rewrite the schema from scratch.** The store is disposable
  (gitignored, 30-day GC, no consumer yet); the `branch` column is simply added
  to the `CREATE TABLE` DDL. Any pre-existing `collector.db` is abandoned.

## Capabilities

### Modified Capabilities

- `conversation-history-store`: `commands` and `files` rows gain a `branch`
  column, derived at hook time from `<gitdir>/HEAD`.
- `kenn-server`: the per-OS state directory is `~/.local/state/kenn/` on macOS
  too (was `~/Library/Application Support/kenn/`).

## Impact

- **Schema:** `branch TEXT` added to `commands` and `files`. No migration code.
- **Hot path:** one extra small file read (`<gitdir>/HEAD`) per capturing hook —
  sub-millisecond, within the ≤5ms budget; no new subprocess.
- **Runtime files relocate on macOS:** a running daemon's old `server.pid`
  becomes orphaned across the upgrade — stop it first; trivial for a per-user
  daemon. No data migration.
