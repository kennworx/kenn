## Why

The collector's `sessions` row holds only id / project / cwd / timestamps /
`last_prompt`. Two useful things are lost:

- **Where the session lives.** A consumer can't jump to the Claude session's
  terminal — there's no record of its tmux pane, so "take me to that session's
  window" is impossible.
- **Session provenance.** The `SessionStart` payload's `source`
  (startup/resume/clear) is parsed and **discarded**, the `transcript_path` (a
  pointer to the session's conversation JSONL) isn't captured, and the OS `user`
  the old JSONL recorded was dropped in the SQLite rewrite.

## What Changes

- **Capture the tmux location** on the `sessions` row: `tmux_pane` (`$TMUX_PANE`,
  e.g. `%5`) and `tmux_socket` (the socket-path field of `$TMUX`). A pane id is
  globally unique across the tmux server, so `tmux switch-client -t %5` reaches
  it **from any other window/session/client**; the socket covers the
  multi-server case. Both come straight from the hook process's environment — no
  `tmux` subprocess, consistent with the collector's payload/env-only ethos.
  Outside tmux they are NULL.
- **Capture session provenance** on the `sessions` row: `source`,
  `transcript_path` (both from the `SessionStart` payload), and `os_user` (from
  `$USER`).
- All five are stamped at `SessionStart`; if another hook created the row first
  (a missed/late `SessionStart`), the fields are backfilled via `COALESCE` and a
  later `SessionStart` does not clobber existing values.

## Capabilities

### Modified Capabilities

- `conversation-history-store`: the `sessions` row gains `source`,
  `transcript_path`, `os_user`, `tmux_pane`, and `tmux_socket`, captured at
  session start from the payload + the hook's environment.

## Impact

- **Schema:** five `TEXT` columns added to `sessions` (rewrite in place, no
  migration — the store is disposable, per the prior change's convention).
- **Hot path:** unchanged — the new fields are read at `SessionStart` only (a
  few `std::env` reads + payload fields), not on the per-Bash hot path.
- **Payload:** `HookInput` gains a `transcript_path` field.
