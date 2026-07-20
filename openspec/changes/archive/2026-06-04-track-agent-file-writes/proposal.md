## Why

`conversation-history-store` captures the agent's `Edit`/`Write`/`Read` tool
touches into a per-session JSONL inbox. But it is **blind to files written
through the shell** — `cmd > out.log`, `… | tee build.log`, `cat > gen.rs
<<EOF`, codegen that emits files. That is how a large share of an agent's
output actually lands, and none of it is recorded. The JSONL inbox is also
**write-only**: nothing consumes it, and the "future ingest pass" the schema
was shaped around was never built.

periClaude — a sibling Claude Code plugin in this author's marketplace — already
solves the shell-capture problem: it walks each Bash command's AST
(`brush-parser`) to extract redirect / `tee` targets, expands variables, and
records them in a queryable SQLite store. This change brings the **same
mechanism** to kenn, generalized to also hold the `Edit`/`Write` touches kenn
already sees, and swaps the unread JSONL for a store something can actually
query.

This is **provenance/history**, orthogonal to index freshness. The `notify`
watcher + `StalenessKey` keep driving reindexing; they answer "is the index
stale?". This store answers "what did the agent write, when, via which
command?".

## What Changes

- **Capture Bash-written files.** A new `PreToolUse(Bash)` hook parses the
  command AST (ported from periClaude's `parser.rs`) for `>`, `>>`, `&>`, and
  `tee` targets, expands `$VAR` / `${X:-…}` against both in-command assignments
  **and** the hook process's ambient environment, and absolutizes against
  `CLAUDE_PROJECT_DIR` / cwd. `PreToolUse` (not just `Post`) so a **long-running
  task's log path is recorded the instant it starts** and can be tailed while it
  runs.
- **Track running state.** `PostToolUse(Bash)` marks the command finished
  (`finished_at`, exit code), linked to its `PreToolUse` row by `tool_use_id`.
  A `finished_at IS NULL` row is a still-running command.
- **Capture Edit/Write as path-only rows.** Keep the existing `PostToolUse`
  touch capture, but store **only the file path** (no `old_string` /
  `new_string` bodies). **Drop `Read`** — it is not a write.
- **Replace JSONL with SQLite.** Write directly from each hook process to a
  **global, project-keyed** SQLite database (`<state_dir>/collector.db`, WAL +
  `busy_timeout`). **No daemon, no network hop** — each short-lived hook opens
  the DB, inserts, and exits. Remove the per-session JSONL inbox, the `ready/`
  markers, and the tagged-union JSON schema.

## Capabilities

### Modified Capabilities

- `conversation-history-store`: switches the capture sink from per-session JSONL
  to a global SQLite store; adds Bash command + output-file capture via AST
  parse; tracks command running-state across `Pre`/`Post`; drops `Read`.

## Impact

- **New hot-path CLI deps:** `brush-parser` and `rusqlite` (both already proven
  in periClaude). Cost is a larger `kenn` binary and a sub-millisecond parse +
  insert per Bash hook — within the existing ≤5ms p95 budget.
- **New crate:** `kenn-collect` — the ported bash parser + the SQLite store.
  `cmd_cc_hook.rs` shrinks to a dispatcher over it.
- **Store moves:** from `<workspace>/.kenn/local/history/*.jsonl` to global
  `<state_dir>/collector.db` (the OS state dir `kenn-server::paths` already
  resolves).
- **Removed surface:** `RawRecord` JSONL records, `ready/` markers, the
  tagged-union line schema, and the `history_*` `Layout` helpers.
- **Hook contract preserved:** graceful failure (any recoverable error → exit 0,
  diagnostic logged, session never interrupted).
- **Consumer deferred:** this change only *collects*. No `kenn history` CLI, MCP
  tool, or findings linkage yet — the schema is shaped to add one later.
