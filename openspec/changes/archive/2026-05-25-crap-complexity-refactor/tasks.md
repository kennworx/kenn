## 1. Bucket A — Lance reader scan fixtures + tests

- [x] 1.1 Add `kenn-store/tests/storage_fixtures.rs` (or expand the existing one) with a helper that materializes minimal `aggregate_nodes`, `aggregate_edges`, `analysis_god_nodes`, `analysis_anchored_communities`, `analysis_flat_communities`, `analysis_node_membership` tables — enough for the readers to return at least one row each. — `build_corpus_with_analysis` (round 2 c95fb1a, extended round 7 0dd3e06).
- [x] 1.2 Test `GraphReader::scan_analysis_god_nodes` (CC=14, 0% → ~14) — round 2.
- [x] 1.3 Test `GraphReader::scan_analysis_anchored_hierarchy` (CC=13) — round 2.
- [x] 1.4 Test `GraphReader::scan_analysis_flat_communities` (CC=11) — round 2.
- [x] 1.5 Test `GraphReader::scan_analysis_node_membership` (CC=8) — round 2.
- [x] 1.6 Test `GraphReader::list_module_files` (CC=7) — round 1 (18adbb9).
- [x] 1.7 Test `GraphReader::fetch_symbol_docs_row` (CC=6) — round 1.
- [x] 1.8 Test `GraphWriter::write_analysis` (CC=13) — round 2 (covered transitively via writer hook).
- [x] 1.9 Test `DbReader::search_symbols_by_name` (CC=13) — round 1.
- [x] 1.10 Test `DbReader::find_similar_symbols` (CC=10) — **partial, accepted as baseline debt**: round 7 added the no-vector-source early-return test; the vector-with-results branch needs the ML embed model in tests, deferred. Still in baseline at CRAP=42. Unblocks when the `embedding-model-update` change lands a testable embed path.
- [x] 1.11 Test `fetch_symbol_embedding` (CC=12) — **deferred, accepted as baseline debt**: needs ML embed path in tests; round 7 commit declared out of scope for unit-level fixtures. Unblocks alongside 1.10 when `embedding-model-update` lands.
- [x] 1.12 Test `edge_kind_from_code` (CC=13, 33% → 13) — round 1, `edge_kind_code_round_trips_every_variant` in schema.rs.

## 2. Bucket B — CLI command runner refactors

- [x] 2.1 Split `cmd_visualize::run` (CC=12) into `parse_args` + `Params` + `execute(params, store) -> Result<()>`; test `execute` against a fixture store. — round 4 (e6723a2) + round 8 (b936016) integration smoke.
- [x] 2.2 Split `cmd_rollback::run` (CC=9) the same way. — round 4.
- [x] 2.3 Split `cmd_index::run_async` (CC=30) into `params` + `execute`; cover `execute` with a smoke test that runs against an empty workspace. — round 4 split + round 8 smoke. `run_async` remains in baseline at cov=73%, CRAP=37: refactored but not fully exercised; remaining branches require live indexer fixtures.

## 3. Bucket C — True complexity refactors

- [x] 3.1 Refactor `render_into` @ `kenn-analyze/src/report.rs` (CC=41 → target ≤ 20): extract per-section renderer methods (`render_god_nodes_table`, `render_anchored_hierarchy`, `render_flat_communities`, etc.). Each method is independently testable. — round 3 (4efcaff).
- [x] 3.2 Refactor `embed_pending` @ `kenn-store/src/db/mod.rs` (CC=25): extract the per-snapshot lock acquisition into a helper so the test path can exercise the "lock contested" / "already embedded" / "embed run" branches separately. — **deferred, accepted as baseline debt**: needs ML embed path; still in baseline at CRAP=33.64. Unblocks alongside 1.10/1.11 when `embedding-model-update` lands.

## 4. Indexer + MCP tool helpers (~8 functions)

- [x] 4.1 `parse_kind` @ `kenn-mcp/src/tools.rs` (CC=24): add cases covering every match arm (the function maps tool-argument strings to `Kind`). — round 1.
- [x] 4.2 `list_usages` + `list_imports` @ `kenn-mcp/src/tools.rs` (CC=8 each): test via the existing integration-test harness against a fixture snapshot. — round 3 (`tests/navigation.rs`).
- [x] 4.3 `transform_document` (CC=16), `StreamState::on_symbol` (CC=11), `StreamState::emit_full` (CC=7), `handle_frame` (CC=16), `ingest_scip_driver` (CC=14), `IndexerDriver::run_all` (CC=10), `derive_edges_for_document` (CC=7), `edge_properties` (CC=13 in edge.rs): add unit tests where the function is pure, otherwise via the integration test that already exercises the pipeline. — rounds 5/6/7 (eefcdbb, 4d2910b, 0dd3e06).
- [x] 4.4 `all_pairs_dijkstra`, `force_layout`, `linlog_layout`, `stress_layout`, `render` in kenn-analyze (CC 8–15): table-driven tests against small fixed graphs. — round 1.
- [x] 4.5 `run_startup_decision` @ `kenn-mcp/src/indexing.rs` (CC=12): test via existing `lifecycle.rs` integration tests if not already covered; otherwise add scenarios for each decision branch. — round 4.

## 5. Verification

- [x] 5.1 Regenerate `crap-baseline.json` and confirm offender count dropped substantially (target: < 10 remaining, all true-complexity). — 43 → 4 (91% remediated). The 4 remaining are all listed in §1.10/§1.11/§2.3/§3.2 deferrals plus `resolve_roots_and_maybe_rebind` added by the mcp-roots-discovery work.
- [x] 5.2 `just crap-ci` passes against the new baseline.
- [x] 5.3 Confirm the gate still bites: verified 2026-05-25. Method: mutated the `DbReader::find_similar_symbols` baseline entry from `crap: 42.24` to `crap: 25.0` (cheaper than reverting a test — same regression-detection code path). `just crap-ci` exited 1 and printed `CRAP gate FAILED: 1 offending entries (regressed or new-over-threshold)` with `status: regressed` for the function. Restored the baseline, re-ran, gate passed.
- [x] 5.4 `cargo clippy --workspace --all-targets` and `cargo test --workspace` clean with all new tests included.
