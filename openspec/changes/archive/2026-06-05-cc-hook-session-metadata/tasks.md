## 1. Schema + store

- [x] 1.1 Add `source`, `transcript_path`, `os_user`, `tmux_pane`, `tmux_socket` (`TEXT`) to the `sessions` `CREATE TABLE` in `kenn-collect::schema` (no migration, design D4).
- [x] 1.2 `kenn-collect::store`: add a `SessionMeta` struct (the five fields, all `Option<String>`) and a `start_session(id, cwd, &SessionMeta, now)` op — INSERT with the metadata; `ON CONFLICT` bump `last_seen_at`, clear `ended_at`, and `COALESCE`-fill each metadata column (design D3). Re-export `SessionMeta`.
- [x] 1.3 Leave `upsert_session` (the ensure-session call used by the other handlers) unchanged so it neither sets nor clobbers the metadata columns.

## 2. Hook wiring

- [x] 2.1 Add `transcript_path: Option<String>` to `HookInput`.
- [x] 2.2 `handle_session_start`: build `SessionMeta` from the payload (`source`, `transcript_path`) + environment (`os_user` = `$USER`; `tmux_pane` = `$TMUX_PANE`; `tmux_socket` = `$TMUX` up to the first `,`) and call `start_session` (design D1, D2).

## 3. Verification

- [x] 3.1 Tests: `start_session` stores all five fields + backfill/no-clobber (COALESCE) — `kenn-collect` store tests; the full env path incl. the `$TMUX`→socket split is covered end-to-end by `cc_hook_smoke::session_start_captures_tmux_and_provenance`.
- [x] 3.2 `cargo clippy --workspace --all-targets` zero warnings.
- [x] 3.3 `just crap-ci` green for touched functions.
- [x] 3.4 `cargo fmt --all` as the final step.
