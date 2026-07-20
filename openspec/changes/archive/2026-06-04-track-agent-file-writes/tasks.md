## 1. `kenn-collect` crate: bash parser

- [x] 1.1 Create the `kenn-collect` crate; add `brush-parser` + `rusqlite` to the workspace deps. `lib.rs` declares/re-exports only. *(rusqlite promoted from a kenn-store direct dep to `[workspace.dependencies]`, bundled; both crates share it.)*
- [x] 1.2 Port periClaude's `parser.rs` into `kenn-collect::parser`: AST walk for `>`, `>>`, `&>`, `tee`; `$VAR` / `${X:-…}` expansion; `signature` (argv0 + first arg). Keep its unit tests. *(`walk_simple` split into `collect_prefix`/`collect_suffix`/`tee_word_target` for the CRAP gate.)*
- [x] 1.3 Extend expansion to consult `std::env` in addition to in-command assignments (design D4). Unresolvable targets keep `resolved=false` + literal text. *(assignment map seeded from `std::env::vars()`; in-command assignments override.)*
- [x] 1.4 Absolutize resolved targets against `CLAUDE_PROJECT_DIR` / cwd. *(`parse(cmd, base)`; absolute passthrough, `~/` via HOME, relative→base+normalize; unresolved kept literal.)*

## 2. `kenn-collect` crate: SQLite store

- [x] 2.1 `kenn-collect::store` opens `<state_dir>/collector.db` (via `kenn_server::paths::state_dir`) with WAL + `busy_timeout` (design D1, D2).
- [x] 2.2 Schema `sessions → commands → files`, `files.command_id` nullable, `channel ∈ {edit,write,redirect,tee}`, `project` on every row; no `confirmed_at`/`size_bytes` (design D5, D6). *(periClaude's `command_signatures`/`parent_tool_use_id`/stat columns dropped — kenn is collect-only.)*
- [x] 2.3 Insert/upsert ops: `upsert_session`, `set_last_prompt`, `insert_command` (Pre), `finish_command` by `tool_use_id` (Post), `insert_file` (edit/write + bash outputs), `end_session`.
- [x] 2.4 Port the 30-day retention + lazy-GC (≤ once / 24h) against the global DB (design D9).

## 3. Hook wiring in `cmd_cc_hook.rs`

- [x] 3.1 Add `pretool-bash` and `posttool-bash` subcommands; capture `tool_use_id` in the payload types (design D3).
- [x] 3.2 `pretool-bash`: parse the command, `insert_command` + parsed output `files`. *(parse is never fatal — on error the command is still recorded with no files.)*
- [x] 3.3 `posttool-bash`: `finish_command` (finished_at + exit code) matched by `tool_use_id`.
- [x] 3.4 `touch`: narrow to `Edit|Write`, store path-only `files` rows; drop `Read` (design D7). *(session row is ensured before the match, so a rejected Read leaves a valid DB with zero `files` rows.)*
- [x] 3.5 `session-start` / `prompt` / `session-end`: write to SQLite (`upsert_session` / `set_last_prompt` / `end_session`).
- [x] 3.6 Delete `RawRecord`, `append_record`, `write_ready_marker`, and the `history_*` `Layout` helpers (design D10). *(verified dead workspace-wide before removal.)*
- [x] 3.7 Preserve graceful failure: any recoverable error → exit 0 + diagnostic log (design D8). *(diagnostic log moved to `<state_dir>/cc-hook.log`.)*

## 4. Install snippet

- [x] 4.1 Update `cc-hook install` to wire `PreToolUse`(Bash) + `PostToolUse`(Bash) + `PostToolUse`(`Edit|Write`); keep `SessionStart` / `UserPromptSubmit` / `SessionEnd`. Idempotent merge preserved.
- [x] 4.2 Update `claude-plugins/kenn/hooks/hooks.json` to match.

## 5. Verification

- [x] 5.1 Tests: bash redirect/tee capture (incl. env-resolved + unresolved); Pre→Post running-state transition by `tool_use_id`; edit/write path-only rows; Read no longer captured. *(kenn-collect 37 tests + cc_hook_smoke 5 tests, all green.)*
- [x] 5.2 Concurrency test: parallel hook writers against one WAL DB don't error.
- [x] 5.3 Hook latency benchmark; record p95, confirm ≤5ms warm (design D8). `tests/latency.rs` (`#[ignore]`) measures the full per-hook in-process work — fresh `Store::open` + parse + `insert_command` + `insert_file`, against a growing DB, as a real short-lived hook does. **Measured (debug build, 2000 warm iters): p50=1.3ms, p95=1.9ms, p99=2.9ms** — within the ≤5ms budget, and release is strictly faster. The dominant cost is the fresh open (WAL + schema `IF NOT EXISTS`), inherent to the no-daemon design (D1). Process spawn itself is fixed overhead the prior JSONL hook also paid and is out of scope.
- [x] 5.4 `cargo clippy --workspace --all-targets` to zero warnings.
- [x] 5.5 `just crap-ci` green for touched functions.
- [x] 5.6 `cargo fmt --all` as the final step.
