## Context

`track-agent-file-writes` shipped the collector: a global `collector.db` under
`kenn_server::paths::state_dir()`, schema `sessions → commands → files`, every
row keyed by `project` (derived from `CLAUDE_PROJECT_DIR` → git toplevel → cwd).
Two gaps motivate this follow-up (see `proposal.md`): no branch dimension, and
the macOS state dir is `~/Library/Application Support/kenn/` rather than the
XDG-style path used on Linux.

## Decisions

### D1 — `branch` is an event-time column on `commands` and `files`

A session can switch branches (`git switch`) between tool uses, so branch is a
property of the *event*, not the session. Add `branch TEXT` to both `commands`
and `files`. `sessions` is left unchanged — a session-level branch would only be
a "last seen" value and the event rows already carry the precise branch. Queries
compose with the existing `project`:

- whole-project history: `… WHERE project = ?1`
- current-branch history: `… WHERE project = ?1 AND branch = ?2`

(`commands` carries `cwd` + `session_id`; its `project` is reachable via the
session join, and its `branch` is now stored directly.)

### D2 — Derive branch by reading `<gitdir>/HEAD`, never a git subprocess

The hook is on the interactive hot path (≤5ms p95 budget, D8 of the prior
change). `git rev-parse --abbrev-ref HEAD` is a 5–15ms subprocess and would blow
that budget. Instead, derive branch by reading `HEAD` directly — sub-millisecond:

1. Walk up from `cwd` for a `.git` entry (the same walk `derive_project` does).
2. If `.git` is a **directory**, the gitdir is `<root>/.git`.
3. If `.git` is a **file** (a linked worktree), it contains `gitdir: <path>`;
   the gitdir is that path (resolved relative to the `.git` file's dir if
   relative). This makes branch correct inside `git worktree` checkouts.
4. Read `<gitdir>/HEAD`. `ref: refs/heads/<name>` ⇒ branch `<name>`. A raw SHA
   (detached HEAD) ⇒ the short SHA. Unreadable/absent ⇒ `NULL` (`resolved=false`
   semantics already exist for paths; branch simply stays NULL).

Derivation lives in the store next to `derive_project` (both take `cwd`), keeping
`cmd_cc_hook.rs` a thin dispatcher — `insert_command` / `insert_file` derive
branch internally, so no hook-side signature churn.

### D3 — `state_dir()` is `~/.local/state/kenn/` on every platform

Replace the `directories`-crate resolution (which yields
`~/Library/Application Support/kenn` on macOS) with: `$XDG_STATE_HOME/kenn` if
set, else `$HOME/.local/state/kenn`. The `$KENN_STATE_DIR` test override is kept
ahead of both. This is the single shared state dir, so `server.pid` /
`server.log` move with `collector.db` / `cc-hook.log`. The config path
(`kenn.toml`, resolved via `config_dir`) is a *separate* resolver and is not
touched.

### D4 — No migration; schema rewritten in place

Per the prototype convention (the store is gitignored, 30-day-GC'd, and has no
consumer yet), `branch` is added directly to the `CREATE TABLE` statements. No
`ALTER TABLE`, no version bump. A pre-existing `collector.db` is simply
abandoned (there is none in practice — the installed hooks still point at the
pre-collector binary, and tests use tempdirs).

## Risks / Trade-offs

- **Worktree HEAD format drift.** Reading `HEAD` assumes the standard
  `ref: refs/heads/…` / raw-SHA format. Anything else ⇒ `branch = NULL`, which is
  the graceful degradation already used for unresolved paths.
- **Orphaned macOS runtime files.** Across the upgrade a running daemon's old
  `~/Library/Application Support/kenn/server.pid` is orphaned. Stop the daemon
  first; no data is lost (state is reconstructable).
