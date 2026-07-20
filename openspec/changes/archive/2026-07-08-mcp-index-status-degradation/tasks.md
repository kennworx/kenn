## 1. Parse the served snapshot's meta at bind time (single site)

- [x] 1.1 In `open_binding` (`crates/kenn-mcp/src/indexing/orchestrate.rs`),
      read `<snapshot_path>/meta.json` into `kenn_indexer::SnapshotMeta`
      (via `read_snapshot_meta`; missing/unparsable → `None`) and add the
      parsed `Option<SnapshotMeta>` to `ReadyParts`.
      **Plan correction:** the review claimed one bind site; there were
      *two* — `open_ready_if_live` (`tools/state.rs`) hand-rolled its own
      open/pin/build-Ready copy. Collapsed it onto `open_binding` +
      `ready_from_parts` (made both `pub(crate)`), so there is now genuinely
      one bind implementation and it inherits the meta parse for free.
- [x] 1.2 `LifecycleState::Ready` gained `run_meta: Option<SnapshotMeta>`;
      threaded through `ready_from_parts`, the in-place `swap_to_snapshot`
      destructure, and the test harness `place_ready`.

## 2. Surface in the payload

- [x] 2.1 Added the five optional fields to `IndexStatus` (`types.rs`), all
      `skip_serializing_if` (`Option::is_none` / `Vec::is_empty` / an
      `is_zero` helper), so a clean run's payload is byte-identical to today.
      The two non-Ready arms pass them empty; only the Ready arm populates.
- [x] 2.2 `build_index_status` populates them via a local `degraded_fields`
      helper: reuses `kenn_indexer::report::render_with_overflow` for the
      lists and `len + overflow` for the counts, and suppresses `run_status`
      for a clean run (`success` + no warnings/failures). `wait_for_index`
      inherits via the flattened `WaitForIndexResponse`.

## 3. Verification

- [x] 3.1 Unit (`tools/lifecycle.rs::degraded_fields_*`): failures+overflow →
      true counts + `+N more`; clean success → omitted; success+warnings →
      warnings surface with `run_status: "success"`; `None` meta → all empty.
- [x] 3.2 Integration (`tests/lifecycle.rs::index_status_reports_partial_run_and_omits_clean`):
      a `partial` meta → `run_status: "partial"` + csharp attribution +
      `failed_count` 5 (1 listed + 4 overflow) + `warning_count` 1, graph
      still serves; a clean `success` meta → all fields omitted.
- [x] 3.3 No metadata read on the status call path: `build_index_status`
      reads only the cached `LifecycleState::Ready.run_meta` (parsed once at
      bind time); `get_index_status` touches no store/git — unchanged.
- [x] 3.4 `cargo clippy --workspace --all-targets` clean; full kenn-mcp
      suite green (85 lib + integration, incl. the two new tests);
      `just crap-ci` green; `cargo fmt --all` last.
