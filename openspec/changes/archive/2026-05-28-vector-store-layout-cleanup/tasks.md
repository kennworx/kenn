## 1. Block A — JSONL stream files into runs/{id}/

- [x] 1.1 In `crates/kenn-indexer/src/driver.rs`, change
  `make_scratch_stream_path` to take the active run dir (or
  run_id + Layout) and return
  `runs/{id}/kenn-dotnet-stream-{pid}-{n}.jsonl`. The
  pid+counter is preserved — it handles retries and concurrent
  shared-`derived_root` configurations.
  — Replaced with `Workspace::jsonl_stream_path(slug)` in
  `crates/kenn-indexer/src/canonicalize.rs`, mirroring the
  existing `Workspace::scip_path(slug)` pattern. When a run dir
  is attached (the real `workflow.rs` pass-init does this), the
  file lands inside it; otherwise falls back to `derived_root/`
  for unit tests. `slug` is the driver's `language_id()` — so
  kenn-dotnet writes to `csharp-stream-{pid}-{n}.jsonl`, not the
  driver-binary-named `kenn-dotnet-stream-*` of before.
- [x] 1.2 Thread the run dir into callers of
  `make_scratch_stream_path`. The indexer pass-init already has
  the run dir; plumb it down through the JSONL driver invocation
  path (`run_jsonl_with_retry` and its callers).
  — Not needed: `Workspace` already carries the run dir via
  `with_run_dir`, set once at pass-init in `workflow.rs`. The
  driver's `run(&workspace)` calls `workspace.jsonl_stream_path(...)`
  directly — no extra plumbing.
- [x] 1.3 Update the existing JSONL-related tests
  (`crates/kenn-indexer/tests/orchestrator.rs`,
  `pipeline.rs::tests`) to construct their fixtures under a
  run-dir-shaped path. Functional behavior unchanged.
  — Not needed: existing tests don't attach a run dir via
  `with_run_dir`, so they hit the legacy `derived_root/` fallback
  (which is the same path they wrote/read before this change).
- [x] 1.4 `cargo test -p kenn-indexer` clean. — 126 tests pass.

## 2. Block B — Findings Lance into runs/{id}/lance/findings/

**Design**: active-run model with publish-fenced lock and a
sync-BM25 / async-vector write path (see `design.md` §B). The
findings Lance lives at `runs/{id}/lance/findings/`, read/written
through `live` — same as `lance/knowledge/`. Vectors survive a
code reindex via the content-addressed findings sidecar.
`store_finding` writes the JSON record and **appends** the row to
`live/lance/findings/` (`WriteMode::Append`) under
`findings-publish.lock`; the BM25/FTS index is sync-searchable on
return via Lance's flat fallback (Open item L, resolved — see the
`fts_finds_appended_unindexed_row` guard); the embedding column is
left NULL. The async embed pass fills finding vectors and runs
`optimize_indices`. The indexer build path repopulates the new
run's findings Lance from records (sidecar vectors) and does a
catch-up scan under the same lock before the live-flip. A finding
is searchable WITHOUT any `kenn index` pass.

- [x] 2.0 Add `Layout::findings_publish_lock_path()` accessor.
  — Landed in commit `0117a73`.
- [x] 2.1 Update `FindingsStore::open` in
  `crates/kenn-store/src/db/findings/store.rs` to resolve the
  Lance mirror under the current live target's
  `lance/findings/`. When `live` is absent, return the same
  "no live snapshot" error code-graph reads return today.
  — `open` no longer builds anything; `dataset()` resolves
  `live_findings_lance_dir()` lazily and returns `no_live_snapshot()`
  when `live` (or its `lance/findings/`) is absent.
- [x] 2.2 Rework `flush()` from full-rebuild to **append + sync
  BM25, null vector** under `findings_publish_lock_path()`: write
  the JSON records, then `WriteMode::Append` the new rows into
  `live/lance/findings/` (embedding column NULL). No re-embed, no
  index rebuild in the write path. When `live` is absent, write
  the JSON only (the next indexer build path materializes the
  Lance). The append must be FTS-searchable on return (guarded by
  `fts_finds_appended_unindexed_row`).
  — `flush()` writes records then, under
  `acquire_findings_publish_lock`, `append_findings_rows` into the
  live mirror (NULL embedding). Skipped when no live run.
- [x] 2.3 Extend the async embed pass to fill NULL finding vectors
  in `live/lance/findings/` (reconcile from the findings sidecar,
  embed misses) and run `optimize_indices` to fold appended
  fragments into the BM25 + vector indexes. Guarded by the run's
  `embed.lock` (Block C).
  — `embed_run_findings` (rebuild-from-records with reconciled
  vectors + fresh indexes, swap under the publish lock) wired into
  `db::embed_pending` after the embed-lock + pin are held; a cheap
  NULL-count gate skips idle ticks.
- [x] 2.4 Build the findings Lance during the indexer pass.
  In the pass that owns `runs/{id}/lance/`, read the committed
  `.kenn/findings/<id>.json` records and build the Lance mirror at
  `runs/{id}/lance/findings/` (reusing sidecar vectors). Acquire
  `findings_publish_lock_path()`, catch-up scan for records that
  appeared since the build started, then let the existing live
  flip publish the run.
  — Exposed `kenn_store::stage_findings_for_publish` (build +
  publish-lock + catch-up, returns the held lock); called before
  `handle.publish()` in both `workflow.rs::run_index` and
  `cmd_index.rs`, lock dropped right after the flip.
- [x] 2.5 Remove `Layout::findings_local_dir()` from
  `crates/kenn-store/src/layout.rs` (task 1.11 from the
  parent change). Confirm no remaining callers via
  `cargo build --workspace`.
  — Removed; replaced by per-run `run_findings_lance_dir(id)` /
  `live_findings_lance_dir()`. A purpose-named `legacy_findings_dir()`
  remains only for the 2.6 startup sweep.
- [x] 2.6 Add startup cleanup: if a stale
  `derived_root/findings/` exists from a prior layout,
  best-effort remove it. Failure is logged, not fatal.
  — `lifecycle::recover` → `sweep_stale_findings_dir` removes
  `legacy_findings_dir()`, logging on failure.
- [x] 2.7 Integration test: `store_finding` writes a record and it
  is keyword-findable immediately against the live run (no index
  pass); after the async embed pass, vector search also returns it.
  — `stored_finding_is_keyword_searchable_without_index_pass`
  (lib) + `flushed_finding_retrieved_by_paraphrase` (hybrid_search,
  vector arm after embed pass).
- [x] 2.8 Integration test: a code reindex creates a new run; the
  finding survives the live-flip (build path repopulated it from
  records, reusing the sidecar vector — no re-embed).
  — `finding_survives_a_reindex_build` (lib).
- [x] 2.9 Integration test: pre-first-index, `store_finding`
  writes the JSON record but a finding-search returns "no live
  snapshot" (or the equivalent code-graph-shaped error).
  — `read_without_live_snapshot_errors` (lib).
- [x] 2.10 `cargo test --workspace` clean.

## 3. Block C — Embed lock into runs/{id}/embed.lock

- [x] 3.1 In `crates/kenn-store/src/db/mod.rs::embed_pending`,
  replace the `Layout::embed_lock_path(snapshot_id)` call with
  `snapshot_dir.join("embed.lock")` — using the `snapshot_dir`
  the function already computes (two parents above
  `lance/knowledge/`).
- [x] 3.2 Remove the lock-file parent-directory creation
  (`create_dir_all(parent)` on what used to be
  `derived_root/embed-locks/`). The parent dir is the run dir,
  which already exists.
- [x] 3.3 Remove `Layout::embed_lock_path` from
  `crates/kenn-store/src/layout.rs` (task 1.13 from the parent
  change). Confirm no remaining callers via
  `cargo build --workspace`.
- [x] 3.4 ~~Add startup cleanup for stale `derived_root/embed-locks/`~~
  — **N/A**: prototype; no deployed users have a stale directory
  to migrate from. Per-run lock dies with `gc()` of the run.
- [x] 3.5 Update any log line / trace that names the old
  `derived_root/embed-locks/<id>` path to name the new
  `runs/{id}/embed.lock` path. — Done in `embed_pending`'s
  doc comment.
- [x] 3.6 Integration / unit test: two concurrent
  `embed_pending` calls against the same run — one wins the
  lock and fills, the other returns the zero-work
  `ReembedReport`. — Existing tests in
  `crates/kenn-store/tests/hybrid_search.rs` and
  `crates/kenn-mcp/tests/background_reindex.rs` simulate the
  contended-lock case; both updated to construct the lock path
  via `snap.join("embed.lock")`.
- [x] 3.7 `cargo test --workspace` clean.

## 4. Verification (whole-change)

- [x] 4.1 `just crap-ci` passes. — "CRAP gate PASSED: no
  regressions, no new over-threshold functions".
- [x] 4.2 `cargo clippy --workspace --all-targets` clean. — Zero
  warnings workspace-wide. Five pre-existing pedantic warnings
  (surfaced by a rust-1.95 lint bump + recent cc-hook work) were
  also fixed: missing doc backticks (`layout.rs`), two
  `semicolon_if_nothing_returned` in `driver.rs`, a
  `map(..).unwrap_or` → `is_ok_and` in `cc_hook_smoke.rs`, and a
  `too_many_lines` `#[expect]` on the `end_to_end.rs` stdio test.
- [x] 4.3 `just embed-smoke` still passes (no regression in the
  in-process llama path). — `llama_embedder_produces_normalized_vectors`
  ok.
- [x] 4.4 Smoke: from a fresh `rm -rf .kenn`, run `kenn index`;
  inspect `.kenn/local/` — no `embed-locks/` directory exists,
  no `findings/` directory exists, the run dir contains
  `embed.lock`, `lance/findings/`, and any `kenn-dotnet-stream-*.jsonl`
  (if a kenn-dotnet pass ran).
  — Ran a fresh `kenn index` on an out-of-repo git workspace (no
  language driver, since the sandbox blocks `rust-analyzer`).
  Inspected `.kenn/local/`: ✅ the published run contains
  `lance/findings/`; ✅ no `embed-locks/` dir; ✅ no derived
  `findings/` dir; the workspace-wide `findings-publish.lock` sits
  at the derived root beside `index.lock` / `live`, as designed.
  NOTE: `kenn index` alone does NOT create `embed.lock` — that file
  is written by the separate embed pass (`embed_pending`), not the
  index pass; and `kenn-dotnet-stream-*.jsonl` only appears when a
  kenn-dotnet pass runs (Block A, covered by `kenn-indexer` tests).
