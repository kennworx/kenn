## Context

`conversation-history-store` today: four `kenn cc-hook` subcommands
(`session-start`, `prompt`, `touch`, `session-end`) each read one Claude Code
hook JSON from stdin and append a tagged-union line to
`<workspace>/.kenn/local/history/<session_id>.jsonl`, plus a `ready/<session_id>`
marker on session end. `touch` matches `Edit|Write|Read`. Nothing reads the
JSONL; the `ready/` marker exists purely for a never-built ingest pass.

Two gaps motivated this change (see `proposal.md`): shell-written files are
invisible, and the sink is write-only. kenn's process topology was surveyed
during exploration:

- **`kenn mcp`** — stdio, one per Claude session, owns the `notify` watcher +
  `StalenessKey` (index freshness). Per-session and stdio-bound — not a
  cross-session collector.
- **`kenn-server`** — a global-singleton HTTP daemon (PID file in the OS state
  dir, `/healthz`, auto-spawned with an idle-timeout), today hosting the
  embedding model.

The exploration weighed a daemon-collector (hooks push events over HTTP) against
direct SQLite writes from each hook. Once parsing landed hook-side (D4), the
daemon's only remaining job was serializing writes — which WAL already does — so
the daemon was dropped (D1). periClaude validates the direct-write model at this
exact event volume.

## Goals / Non-Goals

**Goals:**

- Record files the agent writes through **Bash** (`>`, `>>`, `&>`, `tee`),
  resolved and absolutized, the **same way periClaude does**.
- Know a Bash command's **running state** so a long-running task's log can be
  located and tailed while it is still producing output.
- Record `Edit`/`Write` touches as **path-only** rows.
- Persist to a **queryable, global, project-keyed SQLite** store, written
  directly by each hook with no daemon.
- Preserve the hook contract: never block or interrupt the session.

**Non-Goals:**

- **Confirming a write landed** (stat / size). The store records only what the
  hook payload + AST parse yield; no filesystem reads.
- **A reader.** No `kenn history` CLI, MCP tool, or findings linkage in this
  change — collect now, decide the consumer later.
- **Index freshness.** The `notify` watcher + `StalenessKey` keep that job; this
  store does not trigger reindexing.
- **A daemon / network protocol.** Explicitly rejected (D1).
- **Storing edit bodies** (`old_string` / `new_string`) — path-only (D7).
- **Resolving agent-set shell exports.** A hook is a fresh process; variables a
  Bash tool call `export`s never reach it (D4 caveat).
- **`mv` / `cp` / scaffolding semantics.** Only redirect/`tee` outputs plus the
  free-from-the-walk `signature`; per-command argv semantics are out of scope.

## Decisions

### D1 — Write directly to SQLite from each hook; no daemon

Each short-lived `kenn cc-hook` process opens the SQLite store, inserts, and
exits. Concurrency across simultaneous sessions/workspaces is handled by WAL
journaling + a `busy_timeout` (so concurrent writers retry rather than fail with
`SQLITE_BUSY`). A daemon was considered (route hook events to `kenn-server` over
HTTP) and rejected: with parsing hook-side (D4) the daemon would only serialize
writes, which WAL already provides, and it would add a spawn dance + network hop
to the hot path. periClaude writes SQLite directly from every hook at this event
volume without issue.

### D2 — Global, project-keyed store

The store is a single `collector.db` under the OS state dir resolved by
`kenn_server::paths::state_dir()` (e.g. `~/Library/Application Support/kenn/` on
macOS), **not** per-workspace under `.kenn/`. Every row carries a `project`
column derived from `CLAUDE_PROJECT_DIR` (fallback: git toplevel, then cwd). One
database, one retention/GC policy, queryable across repositories — periClaude's
model. The workspace `.kenn/local/history/` tree is removed.

### D3 — Both `PreToolUse` and `PostToolUse` for Bash; running-state

`PostToolUse(Bash)` does not fire until the command **finishes** — useless for a
multi-minute `cargo test … | tee ./tmp/test.log` whose log you want to tail
*now*. So:

- `PreToolUse(Bash)` parses the command and inserts a `commands` row with
  `started_at` set, `finished_at` NULL, plus the parsed output `files` rows. The
  log path is queryable the instant the command starts.
- `PostToolUse(Bash)` updates the matching row's `finished_at` (and exit code if
  present), located by `tool_use_id`.

`finished_at IS NULL` (and fresh) ⇒ the command is still running — the same
signal periClaude uses for its live view. `tool_use_id` is added to the captured
payload to link the two events.

### D4 — Parse hook-side, with env-enriched expansion

The AST parse (`brush-parser`) runs in the hook process, not a server, so it can
enrich path resolution from the hook's **ambient environment**. Variable
expansion draws from in-command assignments **∪** `std::env`:

```
  OUT=./tmp/x cmd > $OUT          → resolved (in-command assignment; periclaude already)
  cmd > $OUTDIR/run.log           → resolved IF OUTDIR was exported in the launching shell
  export OUT=./x; cmd > $OUT      → NOT resolved (separate Bash shell; never reaches the hook)
```

Env enrichment upgrades `resolved=false → true` for **ambient** vars and stamps
`project` precisely. It does **not** close the agent-set-export gap — a hook is a
fresh process and cannot see a prior Bash call's shell state. Unresolvable
targets are stored with `resolved=false` and the literal text, as periClaude
does.

### D5 — Schema: `sessions → commands → files`, `files.command_id` nullable

periClaude's `sessions → commands → outputs` generalized: `outputs` becomes
`files` with a nullable `command_id` and a `channel`, so one table holds both
Bash outputs and Edit/Write touches.

```sql
sessions(id TEXT PK, project TEXT, cwd TEXT, started_at INT, last_seen_at INT,
         last_prompt TEXT, ended_at INT)
commands(id INTEGER PK, session_id TEXT, tool_use_id TEXT, cmd_text TEXT,
         signature TEXT, cwd TEXT, started_at INT, finished_at INT, exit_code INT)
files(id INTEGER PK, session_id TEXT, command_id INTEGER NULL, project TEXT,
      path TEXT, channel TEXT, op TEXT, resolved INT, t INT)
      -- channel ∈ {edit, write, redirect, tee}
      -- command_id NULL for edit/write touches; set for Bash outputs
```

`channel` lets a future reader filter "what did the agent write via the shell"
vs. "what did it edit"; it is free to record (the parser already knows). No
`confirmed_at` / `size_bytes` (D-NonGoal: no stats).

### D6 — Payload-derived only, no filesystem reads

The collector never stats, opens, or reads a written file. Every column is
derived from the hook JSON + the AST parse. This keeps the hook fast and
side-effect-free beyond the single DB write, and means correctness does not
depend on the file still existing at hook time.

### D7 — Drop `Read`; Edit/Write rows are path-only

The `PostToolUse` touch matcher narrows from `Edit|Write|Read` to `Edit|Write`
("written files"). The row stores `path` + `channel` only — no `old_string` /
`new_string`. Adding a body column later is non-breaking.

### D8 — Preserve the graceful-failure + latency contract

Every recoverable error (malformed JSON, unwritable DB, parse failure, missing
field) is appended to the kenn diagnostic log and the subcommand exits 0 — the
session is never interrupted. The added work (one AST parse + one indexed
`INSERT`/`UPDATE`) stays within the documented ≤5ms p95 budget; a benchmark
gates it, and the install snippet sets `async: true` if realistic p95 exceeds
10ms.

### D9 — Retention / GC ported from periClaude

The 30-day retention + lazy-GC (run at most once / 24h, triggered from a hook)
port over against the global DB, so the store self-bounds without an external
sweeper.

### D10 — Code organization: `kenn-collect` crate

A new `kenn-collect` crate holds the ported `parser.rs` (AST walk + expansion)
and the SQLite store (schema, WAL/busy_timeout init, insert/update/GC). Per repo
convention, `lib.rs` only declares/re-exports; logic lives in named submodules
(`parser`, `store`, `schema`, `gc`). `crates/kenn-cli/src/cmd_cc_hook.rs` becomes
a thin dispatcher: resolve project/env, decode the payload, call `kenn-collect`.
The `RawRecord` enum, `append_record`, `write_ready_marker`, and the `history_*`
`Layout` helpers are deleted.

## Risks / Trade-offs

- **Bigger hot-path binary.** `brush-parser` + `rusqlite` enlarge `kenn`. Accepted
  — periClaude carries both; parse + insert are sub-ms.
- **Global DB write contention.** Many concurrent hooks across sessions write one
  file. WAL + `busy_timeout` mitigate; volume is a handful of writes per agent
  action. periClaude runs this way in practice.
- **Ambient-only variable resolution (D4).** Some `$VAR` redirect targets stay
  `resolved=false`. Acceptable — the literal text is preserved, and the common
  in-command-assignment case already resolves.
- **No confirmation (D6).** A row can name a path the command failed to create.
  Acceptable for a provenance log; the consumer (when built) can reconcile.

## Migration

No data migration: the JSONL inbox was unread, so existing
`.kenn/local/history/*.jsonl` files are simply abandoned (and may be cleaned up
out of band). Users re-run `kenn cc-hook install --write` to pick up the new
hook wiring (adds `PreToolUse`/`PostToolUse` Bash, narrows the touch matcher to
`Edit|Write`).
