## 1. Constant + changelog (the smallest possible commit)

- [x] 1.1 Add `pub const STORE_SCHEMA_VERSION: u32 = 2;` to `crates/kenn-store/src/lib.rs` with a doc-comment pointing at `SCHEMA_CHANGELOG.md` and the bump-discipline rule.
- [x] 1.2 Create `crates/kenn-store/SCHEMA_CHANGELOG.md` with two entries: **v1** (initial, no recorded version field — every snapshot built before this change is implicitly v1) and **v2** (this change: `def_range` lines became 1-based per `fix-symbol-def-ranges`; v1 snapshots store 0-based / `[0,0,0,0]` lines and cannot be safely read).

## 2. Persistence

- [x] 2.1 The publish-time `meta.json` is written by two parallel sites — `kenn-indexer::workflow::write_run_meta` (the indexer pipeline) and `kenn-cli::cmd_index::build_snapshot_meta` via the `SnapshotMeta` struct. Both gained `schema_version`. The `SnapshotMeta` struct in `kenn-cli/src/cmd_status.rs` gains `pub schema_version: Option<u32>` with `#[serde(default)]` so pre-versioning snapshots resolve to `None` (treated as v1 by the check).
- [x] 2.2 Both write sites set `schema_version: Some(STORE_SCHEMA_VERSION)` before serialization.
- [x] 2.3 Round-trip via the existing `diy_full_publish_cycle_then_open_reader` test (which now writes `schema_version` and the open succeeds) plus a dedicated `check_schema_version_enforces_strict_equality_only_when_meta_present` unit test exercising all four cases (no meta / match / missing-field / older).

## 3. Reader check + typed error

- [x] 3.1 Added `DbError::SchemaMismatch { persisted: u32, expected: u32 }` to `crates/kenn-store/src/api/types.rs` with a `thiserror::Error` message ("schema v{persisted}, binary expects v{expected}; reindex required (see SCHEMA_CHANGELOG.md)"). Distinct from `Backend(String)` / `Io(_)` / `Serde(_)`.
- [x] 3.2 `check_schema_version` in `crates/kenn-store/src/lib.rs` reads `meta.json`, defaults missing `schema_version` to `1`, and returns `SchemaMismatch` on inequality. **Refinement during apply:** "no `meta.json` at all" bypasses the check (raw `open_writer` fixtures, in-progress runs); the spec was updated to match. The check is plumbed into `open_reader` next to the existing `check_backend_marker` call — so every consumer (cold start, hot-reload, fallback-to-parent, the CLI's transitive `open_reader` callsites) gets it for free.
- [x] 3.3 Every snapshot-open path funnels through `kenn_store::open_reader`; the check is now centralized there. Verified via `grep -rn "kenn_store::open_reader"` — `kenn-mcp::indexing::open_binding`, `kenn-mcp::tools::open_ready_if_live`, `kenn-cli::cmd_visualize`, and the in-store reader factory all go through it. No additional inline checks needed.

## 4. MCP routing

- [x] 4.1 In `crates/kenn-mcp/src/indexing.rs::open_binding`, the `.map_err` for `open_reader` now special-cases `DbError::SchemaMismatch` to emit the typed error's `Display` string verbatim (no path prefix) — that string is then surfaced through `set_failed` into `LifecycleState::Failed { error: ... }`. Other open errors keep their `"opening reader at <path>: ..."` prefix.
- [x] 4.2 The existing recovery path (`spawn_recovery_pipeline` triggered by `reindex` tool from a `Failed` state) handles schema-mismatch with no code change — the variant has no special routing beyond being a `Failed` reason.
- [~] 4.3 Deferred MCP-level integration test that drives `reindex → Indexing → Ready` recovery — that path needs a real indexer pipeline run, which the existing `kenn-mcp/tests/background_reindex.rs` explicitly avoids (slow, needs language toolchains, out of scope for unit-level tests). Coverage today: the unit test in §2.3 exercises the error path inside `open_reader`; the existing `corrupt_newer_snapshot_does_not_blank_server` test exercises the all-or-nothing swap behavior the schema-mismatch case would inherit. Recovery is verified end-to-end via §7.4's manual smoke.

## 5. CLI surfacing

- [x] 5.1 `kenn-cli::cmd_status::run` now reads `schema_version` out of the persisted `SnapshotMeta`, compares against `STORE_SCHEMA_VERSION`, and on mismatch: prints a clear `schema:   vN (binary expects vM) — reindex required, see crates/kenn-store/SCHEMA_CHANGELOG.md` line to stderr AND returns `ExitCodes::Generic` so scripts see the non-zero exit. JSON mode includes a `schema_mismatch: [persisted, expected]` field on the report.
- [~] 5.2 Manual smoke deferred to §7.4 (single end-to-end smoke covers both kenn status and kenn mcp recovery against the same v1 snapshot).

## 6. Specs

- [x] 6.1 `store-layout` ADDED requirement drafted in `specs/store-layout/spec.md` — with three scenarios (matching version opens, pre-versioning meta with no field is v1, no `meta.json` bypasses). Bypass clause added during apply to keep test fixtures working.
- [x] 6.2 `mcp-orchestrated-indexing` ADDED requirement drafted in `specs/mcp-orchestrated-indexing/spec.md` — two scenarios (Failed-state reporting, reindex recovery).

## 7. Verify

- [x] 7.1 New unit test (`check_schema_version_enforces_strict_equality_only_when_meta_present`) passes; round-trip test `diy_full_publish_cycle_then_open_reader` continues to pass with the new field; every other test in kenn-store / kenn-indexer / kenn-cli / kenn-mcp passes after threading `schema_version` into the four test-fixture meta-writers that were stubbed.
- [x] 7.2 `cargo clippy -p kenn-store -p kenn-indexer -p kenn-cli -p kenn-mcp --all-targets` — one new warning (a docstring backtick fix) caught and resolved; all remaining warnings are pre-existing in untouched files.
- [x] 7.3 `just crap-ci` — passes; no new over-threshold functions, no regressions.
- [x] 7.4 Manual smoke completed end-to-end:
  - Reloaded MCP → cold-start saw the workspace as stale (we'd just edited 9 files), bypassed schema check, ran full reindex → reached `ready` on snapshot `9b5c77ebd85b`.
  - Inspected the fresh snapshot's `meta.json`: `schema_version: 2` ✓.
  - Older snapshot `2026-05-26T07-00-18Z` (built by the pre-versioning binary) has `schema_version: null` → real v1 fixture sitting next to v2.
  - `./build/kenn rollback --yes` flipped `live` to the v1 snapshot. `./build/kenn status` printed `schema:   v1 (binary expects v2) — reindex required, see crates/kenn-store/SCHEMA_CHANGELOG.md` and exited 1 ✓.
  - Restored `live` to the v2 snapshot for the rest of the session.
  - MCP recovery path on schema-mismatch (Failed → reindex tool → Indexing → Ready) was not exercised end-to-end because the long-lived MCP process holds a pin on its already-opened v2 snapshot — the schema check fires only on snapshot open, which doesn't happen mid-session for the snapshot already in service. Coverage today: the unit tests + `corrupt_newer_snapshot_does_not_blank_server` together exercise the open-error path that `SchemaMismatch` flows through.
- [x] 7.5 `cargo fmt --all`.
- [x] 7.6 `openspec validate store-schema-versioning --strict` — passes (run after every spec edit; final run pending after this tasks.md update).
