## 1. Root-cause the miscount

- [x] 1.1 Confirm the symptom on a fresh workspace: `rm -rf .kenn
  && kenn index && jq .documents .kenn/local/runs/*/meta.json`.
  Expect `0` despite non-zero `symbols`. Note the workspace size
  and the language drivers configured — the rust-analyzer
  driver is the primary suspect.
  - Reproduced 2026-05-25 against this worktree (rust + csharp
    drivers): `documents: 0`, `symbols: 538`. The `files` Lance
    dataset reads `output_rows=0` — i.e., zero `FileRecord`s
    were emitted at all. This already rules out hypothesis (C);
    the bug is upstream of the count (the `FileRecord` itself
    is missing), so the `files_seen` plumbing is fine.
- [x] 1.2 Add `eprintln!` at every site that mutates
  `UnitCounts.files` or `RunReport.files_seen`. Sites
  instrumented: `pipeline.rs` SCIP closure (post-`transform_document`),
  the SCIP early-return arm (UnknownLanguage/Excluded/OutsideRoot),
  `pipeline.rs` `finalize_unit`, `pipeline.rs`
  `ingest_jsonl_subprocess` return, `transform_jsonl.rs` `on_file`
  (canonicalize / dup arms), and `workflow.rs` `aggregate_counts`.
- [x] 1.3 Classification: **(B)** — every SCIP doc and every
  JSONL `FileFrame` returned `CanonicalizeError::Excluded`.
  Root cause: `Workspace::canonicalize` rejects via the
  `excluded_dirs` list (populated from
  `discover_other_worktrees`). When this worktree lives at
  `.worktrees/vector-store` *inside* the main repo,
  `discover_other_worktrees` lists the main repo as an "other"
  worktree, and the check
  `for d in &self.excluded_dirs { if abs.starts_with(d) {
  Err(Excluded) } }` matches EVERY file in this worktree (every
  abs path starts with the main repo's path because we live
  inside it). Symbols/defs/edges still tally because the JSONL
  ingest path increments `counts.symbols` from
  `Frame::Symbol` regardless of whether the file passed
  canonicalize — but `counts.files` only bumps when `on_file`
  returned `Some`, so it stayed at 0. (Same shape on the SCIP
  side, but in SCIP the early return drops both — the worktree
  has no rust-analyzer-emitted docs that *succeed* canonicalize,
  so the SCIP unit's c.symbols stayed at 0 too; the 538 symbols
  observed all came from the C# JSONL stream.)

## 2. Fix the root cause

The shape of the fix depends on §1.3. Classification turned out to
be (D) — see 2.4. Branches 2.1/2.2/2.3 (the (A)/(B)/(C) conditionals)
were not taken; they are recorded here as the investigation-time
hypothesis space and superseded by 2.4.

- [x] 2.1 (Not taken — classification was (D), see 2.4.) If (A) —
  the ingest function isn't reached — find the actual ingest path
  the rust-analyzer unit takes and add the equivalent file-count
  increment there. Likely candidates: `ingest_jsonl_into_sink`
  (line ~639) or an earlier dispatch that bypasses
  `ingest_scip_into_sink`.
- [x] 2.2 (Not taken — classification was (D), see 2.4.) If (B) —
  `transformed.file` is None every time — find why the registry
  treats every path as already-seen. The registry is per-pass
  (constructed fresh in `run_pipeline_with_progress`); if it's
  somehow pre-seeded, that pre-seed is the bug. Alternatively, the
  count gate is incorrect — `c.files` should count *distinct paths
  SCIP visited*, not "paths for which we emitted a FileRecord."
  Move the increment to count document iterations, gated on a
  local per-pass `HashSet<file_path>` deduplication.
- [x] 2.3 (Not taken — classification was (D), see 2.4.) If (C) —
  propagation failure — fix the right finalizer. The
  `finalize_unit` function copies `c.files` unconditionally, so
  this case implies `c.files` is being computed in a separate
  scope; reconcile.
- [x] 2.4 Classification revision: this is closer to **(D)** than
  (B). The original (B) hypothesis was "the registry treats every
  path as already-seen" — the actual mechanism is "every path
  fails the workspace exclude check because `excluded_dirs`
  contains the main repo (an ancestor of the worktree root)".
  Fix landed in `crates/kenn-indexer/src/canonicalize.rs`
  `discover_other_worktrees`: skip worktrees that are ancestors
  of `canonical_root` (i.e., `canonical_root.starts_with(&candidate)`).
  This restores correct indexing for any worktree nested inside
  its main repo. `documents:0` was a secondary symptom — the
  primary effect was zero `FileRecord`s emitted at all (the
  `files` Lance dataset stayed empty), which also leaves every
  C# symbol with `file_id = 0` (the JSONL ingest still counts
  symbols/defs/edges even when `on_file` returned None).

## 3. Regression test

- [x] 3.1 Regression covered at the canonicalize layer: extracted
  `should_exclude_other_worktree(canonical_root, candidate)` and
  added three unit tests in `canonicalize.rs`:
  `other_worktree_self_is_skipped`,
  `other_worktree_ancestor_is_skipped` (the regression case),
  `other_worktree_sibling_is_excluded`. A pipeline-level fixture
  test wouldn't have caught this — the bug only surfaces when
  `excluded_dirs` contains a path that's an ancestor of the
  workspace root, which doesn't happen for tempdir-based pipeline
  fixtures. The ancestor-skip unit test is the tightest possible
  regression gate.
- [x] 3.2 Higher-level workflow test deferred: the bug repros
  manually (see §4.3 below) and the unit test in 3.1 pins the
  exact behavior. A workflow test that fakes the nested-worktree
  layout would need to set up a real `git worktree` (the
  discover function shells out to `git`); the cost/value
  tradeoff favors the unit test.

## 4. Verification

- [x] 4.1 `cargo clippy --workspace --all-targets` clean (no
  warnings).
- [x] 4.2 `cargo test --workspace` clean (every reported
  `test result` line shows `0 failed`).
- [x] 4.3 Manual smoke: pre-fix `documents: 0, symbols: 538,
  status: success` (but only C# symbols, files dataset empty).
  After landing the canonicalize fix alone: `documents: 19,
  status: partial` — exposed the D5 dup-symbol panic in §5.
  After landing the §5 fixes too: **`documents: 157,
  symbols: 4425, edges: 6405, status: success,
  failed_projects: []`** on a fresh workspace. Full rust+C#
  index, no panics, both fields non-zero per the spec scenario.
- [x] 4.4 `kenn-dotnet` integration test: `cargo test
  --workspace` includes the kenn-dotnet-relevant tests and ran
  clean; the standalone `just test-indexer-dotnet` xunit suite
  is independent of the Rust changes here (the fix doesn't
  touch C#-side wire parsing).

## 5. Adjacent bug surfaced — scope expansion landed here

Once the worktree-exclude bug was fixed, rust-analyzer's SCIP
ingest actually ran in this worktree (it never did before),
and a different dormant bug surfaced:

```
thread '<unnamed>' panicked at crates/kenn-store/src/db/graph/writer.rs:191:13:
symbols short_id 536871164 written twice — the exactly-once ingest invariant regressed (design D5)
```

User explicitly asked to chase it in this change. Two
duplicate-emit paths existed in `crates/kenn-indexer/src/transform.rs`:

- **5.1** `IdRegistry::intern_with_pub_id` returned
  `is_new = true` whenever the *pub_id* was fresh, even when
  the resolved `short_id` had already been emitted under a
  different pub_id alias. Fix: gate `is_new` on
  `self.full_emitted.contains(&id)` — alias the new pub_id to
  the existing id but tell the caller to skip the emit.
  Regression test:
  `intern_with_pub_id_skips_second_emit_for_same_short_id`.
- **5.2** `IdRegistry::mark_full_emitted` only inserted into
  `full_emitted` and did *not* clear the matching entry in
  `pending_stub_records`. The SCIP path relies on it doing both
  (the in-code comment said "clears the stub via
  `mark_full_emitted`" but the function never did). Without
  the clear, `flush_registry_stubs` at end-of-job emitted the
  stub for an already-emitted short_id and the writer panicked.
  Fix: `mark_full_emitted` now also removes from
  `pending_stub_records`. Regression test:
  `mark_full_emitted_drops_pending_stub_for_same_short_id`.

The intuition behind both: the `IdRegistry` had a clean
"emitted-or-not" tracker (`full_emitted`) but two write paths
weren't consulting it. Both bugs were dormant pre-fix because
the worktree-exclude bug short-circuited the SCIP ingest
before this code ran.
