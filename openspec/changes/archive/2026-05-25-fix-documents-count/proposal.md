## Why

`meta.json` after every `kenn index` pass records `documents: 0`
even when the pipeline ingests hundreds of source files and emits
their `FileRecord`s into the `files` Lance dataset. The bug was
spotted during the `vector-store-layout` change's §9.4 manual
smoke — a real workspace produced:

```json
{
  "timestamp": "2026-05-24T20-06-07Z",
  "status": "success",
  "documents": 0,        // ← always zero
  "symbols": 538,
  "definitions": 538,
  "edges": 2059
}
```

The other counts (`symbols`, `definitions`, `edges`) match
expectations, so the bug is localized to the `files_seen` plumbing
in `kenn-indexer/src/pipeline.rs`. The
`vector-store-layout` change didn't introduce it — `git diff
main..vector-store -- crates/kenn-indexer/src/workflow.rs` shows
zero edits to the `documents`/`files_seen` aggregation. It is a
pre-existing miscount.

The downstream impact is small but real:
- `kenn status` shows `0 documents` regardless of real workload.
- Regression-warning comparisons in
  `cmd_index::compute_regressions` use `prev.documents` vs
  `counts.documents`; the field stays useless until fixed.
- Telemetry / progress reporting in
  `emit_progress(json, "done", …)` includes the count in the
  message body.

## What Changes

### Root-cause the leaf miscount

The probable site (from spelunking, to be confirmed):

`crates/kenn-indexer/src/pipeline.rs:541-542`:

```rust
if transformed.file.is_some() {
    c.files += 1;
}
```

`transformed.file` is `Some(FileRecord)` only when the path is
first seen by `registry.intern_file_with_seen`. Once a path is
in the registry, subsequent SCIP `Document`s referencing it
produce `file: None` (deliberately — to avoid duplicating
`FileRecord` rows in the `files` Lance dataset).

For a single SCIP pass over a fresh workspace, every file should
hit the `is_new_file = true` branch exactly once, so `c.files`
should equal the number of unique source paths in the SCIP
stream. The fact that `documents` reports 0 in production means
either:

1. **The `c.files += 1` site is unreachable** in the live code
   path (maybe a different `ingest_*` function runs and doesn't
   increment, e.g., the JSONL path or a second SCIP path the
   rust-analyzer driver feeds through).
2. **The registry pre-seeds every file** in some setup step, so
   `is_new_file` is always false by the time
   `ingest_scip_into_sink` runs.
3. **The increment is reachable but
   `UnitCounts.files` is not propagated** to `RunReport.files_seen`
   in the unit path the rust-analyzer driver takes —
   `finalize_unit` reads `c.files`, but maybe a different unit
   finalizer (the JSONL one?) overwrites or replaces the
   `RunReport` without copying.

The investigation walks the rust-analyzer SCIP unit's full
ingest path and identifies which of (1)/(2)/(3) — or another
explanation entirely — is the actual cause.

### Fix the miscount

Once root-caused, the fix is expected to be a small targeted
edit in `pipeline.rs` — possibly:

- Move the `c.files += 1` past the file-record dedup gate (count
  unique file paths regardless of whether a `FileRecord` was
  emitted for this doc), OR
- Count files via the registry's existing `is_new_file` signal at
  the right place, OR
- Add the JSONL ingest path's file count to the SCIP path's, if
  the two unit types contribute disjointly.

The right shape falls out of step 1's findings.

### Add a regression test

A pipeline-level test that runs `ingest_scip_into_sink` against a
fixture SCIP file carrying N distinct file paths and asserts
`UnitCounts.files == N`. Lives in
`crates/kenn-indexer/src/pipeline.rs` next to the existing
pipeline tests.

## Capabilities

### Modified Capabilities

- **`index-lifecycle`** — `meta.json` now accurately reports
  `documents` (the count of source files indexed in the pass).
  The field's meaning ("source files visited by the
  indexer") is unchanged; only the implementation is corrected.

### Out of scope

- Renaming `documents` to `files`. The field name is part of the
  on-disk `meta.json` schema and the CLI's `SnapshotMeta` struct;
  changing it is a separate breaking change.
- Rewriting the counts aggregation in `cmd_index::aggregate_counts`
  (it duplicates `workflow::aggregate_counts`; consolidating the
  two is a separate cleanup).
- The JSONL driver's count plumbing (covered by §5.4 deferral of
  the `vector-store-layout` change; if the root cause turns out
  to be JSONL-related, this change either inherits a fix from
  there or punts to it).
