## 1. Test fixtures

Per design conversation, traded strict-TDD integration fixtures for narrower unit-level pins plus the manual smoke (§7.4). End-to-end coverage comes from the live MCP re-probe of this very repo after the binary reloads.

- [~] 1.1 Deferred (Rust integration fixture): the new `transform.rs` unit test `transform_document_populates_def_range_from_definition_occurrence` exercises the 0→1 line basis at the transform boundary directly. Manual smoke (§7.4) covers end-to-end.
- [~] 1.2 Deferred (C# integration fixture): the C# JSONL path's `+1` carve-out is exercised by all existing `transform_jsonl` tests (none regressed). Manual smoke covers the editor-visible `get_source` for `cs:Kenn.Dotnet.*` symbols.
- [~] 1.3 Deferred (find_at_location integration): runtime-side `find_at_location` is basis-agnostic interval logic over stored lines; once ingest is fixed it works without code change. Manual smoke covers this.
- [x] 1.4 Added a wire-format / slice unit test `slice_lines_returns_first_line_for_start_line_1` in `kenn-mcp::tools::tests` — pins that 1-based input yields the first file line (not empty, not skipped). The location-string renderer (`first_def_location_string`) carries a `debug_assert!(start_line >= 1)` for any anchored def, which fences `#0` from the wire in debug builds.

Additional pins beyond the original list:
- [x] `transform_document_emits_placeholder_def_when_no_definition_occurrence` — locks in the zero-range fallback path.
- [x] `transform_document_emits_one_def_per_definition_occurrence` — locks in `scip-indexer` D2.4 (cfg-gated partials produce one row per Definition occurrence with shared `sym_id`).

## 2. Rust SCIP transform — populate def_range

- [x] 2.1 In `crates/kenn-indexer/src/transform.rs`, locate the `Occurrence` with `SymbolRole::Definition` for each symbol while iterating documents. Thread its range to the `DefRecord` push site. (Implemented as a prepass over `doc.occurrences` building a `HashMap<&str, Vec<Range4>>` consumed by the symbol loop.)
- [x] 2.2 Replace the placeholder push at line ~405 with the actual `(start_line, start_col, end_line, end_col)` derived from the definition occurrence, applying `+1` to `start_line` and `end_line` to normalize to 1-based.
- [x] 2.3 Symbols without a `Definition` occurrence in any indexed document keep `def_range = [0, 0, 0, 0]` AND get `is_external = true`. Keep this carve-out for stdlib / dependency references. (Note: the `transform_document` symbol loop only sees symbols in `doc.symbols`, which are workspace-defined by definition. Cross-doc-external symbols are handled by `flush_registry_stubs` via the stub path. The local fallback when a Definition occurrence is missing from this document is a zero-range `DefRecord`; externality remains the job of the stub-flush path.)
- [x] 2.4 Symbols with multiple `Definition` occurrences (e.g., cfg-gated partials) push one `DefRecord` per occurrence. (DefRecord push now happens before the `is_new` dedup so cross-doc partials each contribute.)
- [x] 2.5 Delete the stale "the SCIP path populates the actual range when the def-occurrence is seen later" comment.

## 3. C# JSONL transform — convert to 1-based on ingest

- [x] 3.1 In `crates/kenn-indexer/src/transform_jsonl.rs::def_for`, add `+1` to `start_line` and `end_line` when constructing the `DefRecord`. Columns pass through unchanged.
- [x] 3.2 Carve-out: if `s.range == [0, 0, 0, 0]` (the synthetic-symbol case per `dotnet-stream-indexer`), store `[0, 0, 0, 0]` as-is — do NOT add `+1`. Detect by checking all four values are zero before applying the adjustment.

## 4. Reader — remove the off-by-one heuristics

- [x] 4.1 In `crates/kenn-mcp/src/tools.rs::slice_lines`, remove the `start_line.max(1)` heuristic. With 1-based input the function becomes a straight `skip(start - 1).take(end - start + 1)`. Document that the function expects 1-based input. (Added a `debug_assert!(start_line >= 1)` and 1-based docstring.)
- [x] 4.2 Audit the `<path>#<start>-<end>` rendering helper (`first_def_location_string` in `tools.rs`). Confirms it passes the stored value as-is. Added a `debug_assert!(first.start_line >= 1)` for anchored (`file_id != 0`) defs so a producer regression surfaces in debug builds rather than rendering `#0` on the wire.
- [x] 4.3 Audit `crates/kenn-indexer/src/edge.rs::DocumentDefIndex::smallest_enclosing` — this operates on raw 0-based SCIP occurrence ranges during edge derivation and never touches the stored `defs` table. The runtime-side `find_at_location` in `kenn-store::db::graph::reader` uses simple `start <= line && line <= end` interval logic that is basis-agnostic so long as input and stored values share a basis (they now do — both 1-based). No code change needed.
- [x] 4.4 In `crates/kenn-mcp/src/server.rs` (around line 130), flip the `find_at_location` tool description from "0-based line number" to "1-based line number" so the agent-visible contract matches the new stored basing. Also annotated the `FindAtLocationArgs.line` field with a 1-based docstring so the field-level schema description matches.
- [~] 4.5 Deferred per "skip TDD" directive. The live MCP smoke (§7.4) demonstrated the 1-based round-trip end-to-end — `find_at_location tools.rs#294` returned the right enclosing symbol and `tools` module rendered as `tools.rs#1-2191` (a `#1` start that the old 0-based world could never produce). Reader-side `slice_lines_returns_first_line_for_start_line_1` pins the 1-based interpretation at the unit level.

## 5. Specs — update text

- [x] 5.1 Apply the `source-data-model` ADDED requirement: "def_range line basing is 1-based, column basing is 0-based". (Already drafted in `specs/source-data-model/spec.md`.)
- [x] 5.2 Apply the `source-data-model` MODIFIED requirement: clarify wire location format renders 1-based lines as-is.
- [x] 5.3 Apply the `scip-indexer` ADDED requirement: "def_range Is Populated for Every Non-External Symbol".

## 6. Snapshot invalidation — DEFERRED

The proposal assumed a `SNAPSHOT_GENERATION` / schema-version marker that does not exist today (staleness is workspace-content-based via git HEAD + dirty hashes, not store-schema-based). Adding such a mechanism is a generic capability that warrants its own change. Users running `kenn` after this fix lands must reindex manually (`rm -rf .kenn/` then `kenn index`) until that follow-up.

- [~] 6.1 Deferred to a future `store-schema-versioning` change.
- [~] 6.2 Deferred (depends on 6.1).

## 7. Verify

- [x] 7.1 Run the new tests from §1 — all pass. (3 new transform.rs tests + 1 slice_lines test all green; full `cargo test -p kenn-indexer -p kenn-mcp -p kenn-store` clean — no regressions across 27 suites.)
- [x] 7.2 `cargo clippy --workspace --all-targets` — zero new warnings on touched code. (6 pre-existing pedantic warnings in `driver.rs` semicolons and `end_to_end.rs::too_many_lines` are unchanged from baseline; CLAUDE.md §3 says don't touch adjacent unrelated warnings.)
- [x] 7.3 `just crap-ci` — passes after extracting `collect_definition_occurrences` and `push_def_records` helpers; the inlined version pushed `transform_document` to CRAP=35 (cyc=27, cov=77%), the refactor pulls it back under the 30 threshold without baseline churn.
- [x] 7.4 Manual smoke: live MCP probe against snapshot `de7eaf781ebf` (rebuilt with the fix):
  - `get_source rs:kenn-mcp::tools::ServerState::clear_result_caches` → `{start_line: 294, end_line: 294, text: "    pub fn clear_result_caches(&self) {"}` ✓
  - `get_source cs:Kenn.Dotnet.Cli.IndexCommand` → `{start_line: 16, end_line: 16, text: "public static class IndexCommand"}` ✓
  - `find_at_location crates/kenn-mcp/src/tools.rs#294` (1-based) → returns `clear_result_caches` as tightest enclosing + `tools` module (`tools.rs#1-2191`). The `#1` start is the cleanest evidence — the old 0-based world could never produce that.
- [x] 7.5 Run `openspec validate fix-symbol-def-ranges --strict` and confirm the change is archive-ready. — Valid.
