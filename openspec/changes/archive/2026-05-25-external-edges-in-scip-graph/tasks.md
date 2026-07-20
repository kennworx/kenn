## 1. Edge-gate change

- [x] 1.1 In `crates/kenn-indexer/src/edge.rs:148-151`, drop the `== 0` arm of the gate. The remaining check is `if target_def_count > 1 { continue; }`. Update the docstring above the function to reflect that `def_count == 0` references are now emitted (target gets stubbed via the existing `intern_symbol_with_stub` call).
- [x] 1.2 Update `derive_edges_for_document`'s test `derive_edges_for_document_skips_when_target_def_count_not_one`: now asserts only the `>1` drop. Add new test `derive_edges_for_document_emits_when_target_has_zero_defs` verifying the edge IS emitted with the external target stubbed.

## 2. Drain-time external tag

- [x] 2.1 In `crates/kenn-indexer/src/transform_jsonl.rs:267` (`flush_registry_stubs`), set `rec.external = true` on each drained stub before pushing to the sink. Update the function's docstring to reflect the new invariant.
- [x] 2.2 Unit test: after a run where one stub never receives a full SymbolFrame, the persisted SymbolRecord for that stub has `external = true`.
- [x] 2.3 Unit test: a cross-document workspace symbol that gets upgraded mid-run lands in the sink with `external = false` (verifies `mark_full_emitted` still wins over the drain-time tag).

## 3. End-to-end integration

- [x] 3.1 Integration test: a fixture Rust workspace that uses `Result::unwrap`, indexed end-to-end, produces a symbol row for `core::result::Result::unwrap` with `external = true` and at least one inbound `calls` edge from a workspace function. — `external_symbol_lands_with_external_true_and_inbound_edge` in `tests/orchestrator.rs`.
- [x] 3.2 Verify `find_symbol("unwrap", include_external = false)` returns empty AND `find_symbol("unwrap", include_external = true)` returns the external row on the same snapshot. — covered inline at the bottom of the §3.1 test using `Reader::find_symbol_tiered`.

## 4. Quality gate

- [x] 4.1 `cargo clippy --workspace --all-targets` shows zero warnings (per CLAUDE.md §5).
- [x] 4.2 `just crap-ci` passes — fix any new over-threshold function YOU introduced (per CLAUDE.md §6); do not blanket-rebaseline.
- [x] 4.3 Update `.kenn/` snapshots in any test fixture that records edge/symbol counts so the regression baselines reflect the new behavior. Expected delta direction: external symbol rows and edge rows both grow; workspace symbol counts unchanged. — `cargo test --workspace` all 45 test blocks pass; no fixture asserts on absolute counts that would break with the +73 % edge growth (existing assertions use shape checks and language partitioning).

## 5. Documentation

- [x] 5.1 Add a brief note in the user-facing docs explaining that external (stdlib / vendored) references now appear in the index and are filterable via `include_external`. No config knob exists; the behavior is unconditional. — `claude-plugins/kenn/skills/kenn/SKILL.md` Tools section gained an "External symbols" paragraph.
- [x] 5.2 Update any MCP-tool documentation that describes `include_external` parameter behavior to note that it now affects SCIP-language results. — added a doc-comment to the shared `Filters` struct in `crates/kenn-mcp/src/types.rs` so every tool that takes filters inherits the explanation.
