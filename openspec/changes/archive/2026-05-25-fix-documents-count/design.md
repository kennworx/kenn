# Design

The proposal framed three hypotheses (A/B/C) for why `meta.json.documents`
was always `0`. Investigation (§1.3 of tasks) showed the real shape was
neither — every path failed `Workspace::canonicalize` with `Excluded`,
because `discover_other_worktrees` listed the main repo as an "other
worktree" of a nested worktree, and the exclude check treated *any*
descendant of that main repo as excluded. The fix lives at the
canonicalize layer, not in the count plumbing.

Two design decisions shipped in this change.

## D1: Ancestor-worktree skip in `discover_other_worktrees`

`Workspace::canonicalize` rejects any path whose absolute form starts with
an entry in `excluded_dirs`. `excluded_dirs` is populated from
`discover_other_worktrees`, which shells out to `git worktree list`. The
discover function did not filter the *current* workspace's ancestors out
of that list, so when the workspace itself was a worktree nested inside
its main repo (e.g. `<repo>/.worktrees/foo`), the main repo appeared as
an "other worktree" and every path under the workspace root was a
descendant of it — so every path was excluded.

**Fix:** skip any candidate `c` for which `canonical_root.starts_with(&c)`
holds. Extracted as a pure helper `should_exclude_other_worktree(root,
candidate)` so the regression cases unit-test cleanly:

- `other_worktree_self_is_skipped` — the workspace itself doesn't
  exclude itself.
- `other_worktree_ancestor_is_skipped` — the regression case.
- `other_worktree_sibling_is_excluded` — peer worktrees still excluded
  (the original purpose of the list).

A pipeline-level fixture test was deferred (§3.2): the bug only
materializes when the workspace lives inside another git worktree's
tree, which `tempdir`-based pipeline fixtures don't reproduce, and
faking the layout would require a real `git worktree add`. The unit
test on the extracted helper pins the exact pre-condition.

## D2: `IdRegistry` exactly-once invariant restoration (design D5)

Once D1 stopped short-circuiting the rust-analyzer SCIP ingest in the
nested-worktree case, a different dormant bug surfaced as a graph-writer
panic: `symbols short_id N written twice — the exactly-once ingest
invariant regressed`.

Two write paths weren't consulting the registry's `full_emitted`
tracker:

- **`intern_with_pub_id`** returned `is_new = true` whenever the
  *pub_id* was fresh, even when the resolved `short_id` had already
  been emitted under a different pub_id alias. The caller used
  `is_new` to decide whether to emit a row.
  *Fix:* gate `is_new` on `!self.full_emitted.contains(&id)` — the
  caller now aliases the new pub_id but skips the emit.

- **`mark_full_emitted`** inserted into `full_emitted` but did not
  clear the matching entry in `pending_stub_records`. The SCIP path's
  in-code comment claimed it cleared the stub, but the function never
  did. `flush_registry_stubs` at end-of-job then emitted the stub for
  an already-emitted `short_id`, tripping the writer's exactly-once
  assert.
  *Fix:* `mark_full_emitted` now also removes from
  `pending_stub_records`.

Regression tests pin both:
`intern_with_pub_id_skips_second_emit_for_same_short_id` and
`mark_full_emitted_drops_pending_stub_for_same_short_id`. Both bugs
were latent pre-fix because the worktree-exclude bug (D1)
short-circuited the SCIP ingest before this code ran on any nested
worktree.

## Why the proposal's `pipeline.rs:541-542` hypothesis was wrong

The proposal pointed at `c.files += 1` gated on `transformed.file.is_some()`,
guessing the registry-dedup gate or a propagation gap was the issue.
The actual mechanism was upstream of that line: `transform_document`
returned `Err(CanonicalizeError::Excluded)` for *every* SCIP document,
so the increment site was never reached for any document. The
`documents: 0` symptom was a downstream consequence of the
`FileRecord`s never being produced at all — confirmed by inspecting the
`files` Lance dataset (`output_rows = 0`). The count plumbing in
`pipeline.rs` is correct; the workspace canonicalizer was the bug.
