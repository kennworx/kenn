## 1. git abstraction backed by gix

- [x] 1.1 Add `gix` to the workspace with `default-features = false` + only
      refs/worktree/status features (no network/HTTP). Record the `just build-cli`
      time + `./build/kenn` size delta (design D4).
      → `gix 0.85.0`, `features = ["status", "max-performance-safe", "sha1"]`.
      Network HTTP clients (curl/reqwest) excluded, but `gix-protocol`/`transport`/
      `packetline`/`refspec` are **unconditional deps of the `gix` facade** — not a
      feature leak. Weight: **+69 crates** (351→420). Binary-size delta recorded
      below (§3.4).
- [x] 1.2 Add a `kenn-store` git module exposing: `head_id`, `tracked_modified`
      (sorted paths incl. deletions, deduped), `main_worktree`, `all_worktrees`,
      and `work_dir`. Opening a non-repo returns the "not git" signal (`None` /
      empty) the call sites already handle.
      → `crates/kenn-store/src/git.rs`. `main_worktree` = canonicalized
      `common_dir` parent (no worktree enumeration). `tracked_modified` iterates
      `status().untracked_files(None)` over BOTH tree-index (staged) and
      index-worktree (unstaged), tracked only.

## 2. Port call sites

- [x] 2.1 `staleness.rs` — replace `git rev-parse HEAD` + `git status --porcelain`
      with the module. **Preserve exactly**: tracked-modified only, no untracked
      reads, deletion sentinel (design D2).
      → `git::head_id` + `git::tracked_modified`; `dirty_entry` still maps a
      deleted path to the `DELETED_OR_UNREADABLE` sentinel.
- [x] 2.2 `worktree.rs` — replace `git worktree list --porcelain` /
      `resolve_main_worktree` with the module's `main_worktree`. (Deleted the
      orphaned `parse_main_worktree_path` porcelain parser + its test.)
- [x] 2.3 `canonicalize.rs` (`discover_other_worktrees` → `git::all_worktrees`),
      `main.rs` + `cmd_cc_hook.rs` (`git_toplevel` → `git::work_dir`).
- [x] 2.4 Removed `Command::new("git")` from all runtime code (verified by grep);
      remaining calls are `#[cfg(test)]` fixture setup only.

## 3. Verification

- [x] 3.1 **Parity gate (D2):** `staleness::tests::gix_tracked_set_matches_git_
      porcelain_and_ignores_untracked` — gix set == `git status --porcelain` set
      (mods + deletes + nested), a 50-file untracked `node_modules/` never appears.
      Plus `a_staged_modification_changes_the_key` (proves the tree-index/staged
      comparison is included) and `a_staged_rename_changes_the_key`.
- [x] 3.2 Worktree resolution + parent-fallback tests pass unchanged (all 6
      `worktree::tests` + 3 `worktree_discovery` integration tests).
- [x] 3.3 `tests/gix_no_git_binary.rs` — staleness (git form) + `resolve_main_
      worktree` succeed after `PATH` is cleared. Own test binary so the
      global-env clear races with nothing.
- [x] 3.4 `cargo clippy --workspace --all-targets` clean; `just crap-ci` green;
      `cargo fmt --all` last.
      → clippy clean (fixed gix `work_dir`→`workdir` deprecation); CRAP green.
      **D4 binary size (release, stripped `kenn`): 15.8 MiB → 17.7 MiB = +1.9 MiB
      (+12%)** — modest, since lto+strip+DCE drops gix's unused network paths.
      Build-*time* cost is the real one: +69 crates (351→420). Acceptable; no D3
      fallback. `cargo fmt --all` last.
