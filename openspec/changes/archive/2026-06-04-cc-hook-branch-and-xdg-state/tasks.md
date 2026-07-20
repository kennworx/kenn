## 1. Branch capture

- [x] 1.1 Add `branch TEXT` to the `commands` and `files` `CREATE TABLE` DDL in `kenn-collect::schema` (no migration — rewrite in place, design D4).
- [x] 1.2 `kenn-collect::store`: add `derive_branch(cwd)` (+ `find_gitdir`) next to `derive_project` — walk up for `.git`, handle `.git`-as-a-file worktree (`gitdir:` pointer), read `<gitdir>/HEAD`, map `ref: refs/heads/<name>` → name, raw SHA → short SHA, else `None` (design D2).
- [x] 1.3 Thread branch into `insert_command` and `insert_file` (derived internally from `cwd`, like `project` — no hook-side signature change).

## 2. State dir on macOS

- [x] 2.1 `kenn_server::paths::state_dir()`: resolve `$KENN_STATE_DIR` → `$XDG_STATE_HOME/kenn` → `$HOME/.local/state/kenn` on Unix (Linux+macOS); Windows keeps the `directories` resolution. Module doc comment updated (design D3).

## 3. Verification

- [x] 3.1 Tests: a `commands`/`files` row carries the current branch; detached HEAD → short SHA; non-git cwd → NULL branch; a linked-worktree (`.git`-file) checkout resolves its branch (`kenn-collect` store tests, 5 added → 42 total).
- [x] 3.2 Test: `state_dir()` resolves under `~/.local/state/kenn` on Unix and never `~/Library/Application Support`. The `$KENN_STATE_DIR` override is covered end-to-end by the subprocess-based `cc_hook_smoke` tests (in-process env mutation is racy, so it isn't unit-tested here).
- [x] 3.3 `cargo clippy --workspace --all-targets` zero warnings.
- [x] 3.4 `just crap-ci` green for touched functions.
- [x] 3.5 `cargo fmt --all` as the final step.
