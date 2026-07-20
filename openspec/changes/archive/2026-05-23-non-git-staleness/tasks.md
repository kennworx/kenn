## 1. Generalize the staleness key

- [x] 1.1 Change `StalenessKey` (in `kenn-store/src/staleness.rs`) to an enum: `Git { head, dirty_files }`, `Tree { fingerprint: u64 }`, `Unknown`. Keep `DirtyFile` as-is.
- [x] 1.2 Rewrite `StalenessKey::matches` — true only for same-form equal keys (`Git`↔`Git`, `Tree`↔`Tree`); every mixed or `Unknown` pairing is false.
- [x] 1.3 Unit-test `matches`: equal tree keys match; differing tree keys don't; a git key and a tree key never match; `Unknown` never matches.

## 2. The tree fingerprint

- [x] 2.1 Add a `stat`-only depth-first tree walk in `staleness.rs` that folds each file's `(workspace-relative path, mtime, size)` into an `xxh3-64` digest in a deterministic order; skip the fixed directory set `node_modules`, `target`, `bin`, `obj`, `.git`, `.kenn`. Never read file contents.
- [x] 2.2 `compute_staleness_key`: return `Git { .. }` when `git rev-parse HEAD` succeeds (today's path); otherwise return `Tree { fingerprint }`; return `Unknown` only if the tree walk itself fails.
- [x] 2.3 Unit-test the fingerprint: editing a file changes it; an unchanged tree is stable across calls; writing under `.kenn/` does not perturb it.

## 3. Route the freshness decision through the generalized key

- [x] 3.1 Remove the non-git `follow_live` branch from `decide_startup_state` (added by `config-driven-store-layout`) — a non-git workspace now carries a `Tree` key and flows through the scan-by-key path. `Unknown` still falls through to `Reindex`.
- [x] 3.2 Remove the `staleness.git_head.is_some()` skip guard from `cmd_index` — an unchanged non-git workspace now has a matchable key and may legitimately skip.
- [x] 3.3 Confirm snapshot `meta.json` records the new key shape and `decide_startup_state` / `live_knowledge_dir` read it back; a pre-change snapshot's key never matches a `Tree` key and triggers one reindex.

## 4. Verification

- [x] 4.1 Integration test: a non-git workspace — `kenn index` then re-run skips (unchanged); edit a file and it re-indexes; the MCP server and `kenn embed` resolve the matching snapshot.
- [x] 4.2 Regression test: git workspaces are unaffected — the git key path and `matches` behave exactly as before.
- [x] 4.3 `cargo clippy --workspace --all-targets` is clean.
