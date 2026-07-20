## 1. Phase 1 — relative location resolves at the git root

- [x] 1.1 In `resolve_location_spec` (`crates/kenn-store/src/layout/resolve.rs`),
      the relative arm resolves against an `anchor_root` param — the main
      worktree (via `vectors_anchor` → in-process gix `git::main_worktree`)
      for `[vectors] location`, the source root (unchanged) for
      `[layout] derived_root`. Falls back to `source_root` outside a git tree,
      and preserves the caller's own path spelling when it *is* the main
      worktree (so default layouts compare equal to the pre-change paths).
- [x] 1.2 Test (`worktree.rs::relative_vectors_location_resolves_at_the_main_worktree`):
      a linked worktree and the main worktree with `[vectors] location =
      "team-vectors"` resolve to the same dir; non-git dirs unchanged
      (existing `layout::types` tests still pass untouched).

## 2. Phase 2 — multi-generation store + GC

- [x] 2.1 Generation namespace `<vectors_root>/<model>/<dim>/<quant>/<recipe>/`
      (`embed/sidecar/generation.rs`; recipe tags like `doc/v1` nest, model ids
      sanitized for portability). Writers (`jobs.rs`, `findings/embed.rs`) and
      the finalize reuse read target the generation dir; the legacy flat
      `code/`/`findings/` dirs stay readable as a same-generation fallback
      (`load_reuse_map_with_legacy`) so committed `pack-*.bin` files keep
      serving fresh clones with no migration.
- [x] 2.2 **`reset_vectors` deleted**; the recipe-mismatch wipe branch and the
      model-mismatch hard error in the embed passes are gone — a recipe/model
      change targets a new generation dir and old ones stay intact.
- [x] 2.3 `load_reuse_map` gained an `expected_model` gate (defense-in-depth on
      top of the path separation); `FindingsStore` reads carry the configured
      model id. Test: `load_reuse_map_rejects_model_mismatch`.
- [x] 2.4 GC (`gc_vector_cache`): per-generation `.last-access` stamp touched on
      reuse reads and appends; LRU eviction past `[vectors] cache_cap_mb`
      (default 1024, `0` disables); never evicts the active generation
      (manifest-matched, so the legacy dir is protected exactly while it still
      serves the active generation) nor a dir holding committed `pack-*.bin`
      (design D6); `gc.lock` flock scoped to the GC pass (appends stay
      lock-free). Triggers: lazily at embed-pass start (shared by the CLI and
      workflow/MCP paths) and explicitly via the new `kenn gc` command.
- [x] 2.5 Tests: `generation_switch_leaves_prior_vectors_intact_and_reusable`
      (switch back = reuse map non-empty, zero re-embed),
      `gc_evicts_lru_inactive_generation_and_keeps_active`,
      `gc_never_evicts_pack_holding_generations`, `gc_under_cap_is_a_noop`,
      `legacy_dir_serves_reuse_alongside_the_generation_dir`.

## 3. Phase 3 — shared default (gated on Phase 2)

- [x] 3.1 D5 decision: **option A** — the shared default is the main worktree's
      *committed* `.kenn/vectors`, preserving the committed-sidecar property
      (fresh clone searches offline via committed packs — served through the
      legacy fallback and future generation-dir packs). The D6 lean is
      implemented literally: GC never touches pack-holding dirs.
- [x] 3.2 Default `vectors_root` (both `Layout::default_for` and
      `Layout::resolve`) anchors at the main worktree; for the main worktree
      itself and non-git dirs the path is byte-identical to before.
- [x] 3.3 Test (`worktree.rs::linked_worktree_default_layout_shares_the_main_vectors_root`):
      a linked worktree's default layout points at the main tree's vectors dir
      out of the box while its derived store stays per-worktree. Concurrent
      cross-worktree writes are safe by the existing content-addressed
      atomic-append protocol (idempotence + atomic-rename io tests).

## 4. Verification

- [x] 4.1 Cross-checkout embed-once: `fresh_worktree_reuses_committed_vectors`
      (hybrid_search.rs, real model) — a second store reconciling against the
      first checkout's generation dir re-embeds **zero** symbols.
- [x] 4.2 No destructive wipe on a generation change (task 2.5 tests; no wipe
      code path remains in the tree).
- [x] 4.3 `cargo clippy --workspace --all-targets` clean; `just crap-ci` green;
      `cargo fmt --all` last.
