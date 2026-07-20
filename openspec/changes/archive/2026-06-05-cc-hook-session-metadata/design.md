## Context

`track-agent-file-writes` stores a minimal `sessions` row (id / project / cwd /
timestamps / `last_prompt`); `cc-hook-branch-and-xdg-state` added per-event
branch. This change enriches the **session** row so a consumer can (a) jump to
the session's terminal and (b) know its provenance + conversation transcript.

## Decisions

### D1 — tmux location is `$TMUX_PANE` + the `$TMUX` socket, env-only

A process started inside tmux inherits `$TMUX_PANE` (the pane id, e.g. `%5`) and
`$TMUX` (`<socket-path>,<server-pid>,<session-id>`). The hook reads both from its
own environment — no `tmux` subprocess. The pane id is **globally unique across
the tmux server**, so it is a complete switch target: `tmux switch-client -t %5`
(optionally `tmux -S <socket> …` for a non-default server) focuses the session's
window from any other client. Store `tmux_pane` = `$TMUX_PANE` and `tmux_socket`
= the substring of `$TMUX` before the first `,`. Both NULL outside tmux.

### D2 — Provenance fields from the payload + env

- `source` — the `SessionStart` payload field (`startup` / `resume` / `clear` /
  `compact`); already decoded into `HookInput` but currently discarded.
- `transcript_path` — added to `HookInput`; the payload's path to the session's
  conversation JSONL. Stored as the pointer, not the content.
- `os_user` — `$USER` from the hook environment (restores the old JSONL `user`).

Named `os_user` (not `user`) to avoid any ambiguity with SQL and to be explicit
that it is the OS account, not a Claude identity.

### D3 — A dedicated `start_session`; lightweight `upsert_session` unchanged

`handle_session_start` gathers the five fields into a `SessionMeta` and calls a
new `Store::start_session(id, cwd, &meta, now)`. It INSERTs the row with the
metadata; on `ON CONFLICT` it bumps `last_seen_at`, clears `ended_at`, and
`COALESCE`-fills each metadata column (so a row a prior ensure-call created with
NULLs is backfilled, and a duplicate `SessionStart` never overwrites a value
with NULL).

The existing `upsert_session(id, cwd, now)` — the ensure-session call every other
handler makes — is **unchanged**: it never writes the metadata columns, so it
neither sets nor clobbers them. This keeps the non-start hooks untouched and the
metadata authoritative from `SessionStart`.

### D4 — No migration

Per the prototype convention, the five columns are added to the `sessions`
`CREATE TABLE` directly; any pre-existing `collector.db` is abandoned.

## Risks / Trade-offs

- **tmux env not inherited.** If Claude Code was not launched inside tmux (plain
  terminal, IDE), `$TMUX_PANE` is unset ⇒ NULL tmux fields. Expected; capture
  otherwise proceeds.
- **Pane id reuse after server restart.** Pane ids are unique per tmux *server*
  lifetime; a tmux server restart can recycle `%N`. A stored pane id is only
  meaningfully switchable while that server lives — acceptable for a 30-day,
  best-effort provenance log; a consumer can verify the pane exists before
  switching.
