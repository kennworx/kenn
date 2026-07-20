## Why

`fix-symbol-def-ranges` (2026-05-26) changed the meaning of stored `def_range` values (0-based → 1-based) without any mechanism to invalidate snapshots written by prior binaries. A user on the new binary opening a `.kenn/` dir built last week sees `is_stale = false` (the workspace itself hasn't changed) and silently gets wrong `get_source` output. The bug-fix change deferred this gap because it's a generic capability, not specific to that one schema change.

This change adds a single store-schema-version integer that every snapshot publishes and every reader checks on open. A mismatch fails fast with a clear "reindex required" error instead of serving corrupt-by-definition data.

## What Changes

- Add `STORE_SCHEMA_VERSION: u32` constant in `kenn-store`, initialized to `2` (anything pre-existing is implicitly `1`, treated as a mismatch).
- Persist `schema_version: u32` in each published snapshot's existing metadata (`meta.json` in the snapshot run dir; the file already exists for `indexed_at` and friends).
- On every snapshot open path (cold start, hot-reload after reindex publish, fallback-to-parent-worktree), compare the stored version to `STORE_SCHEMA_VERSION`. Mismatch → return a typed `SchemaMismatch` error that the lifecycle layer maps to `LifecycleState::Failed`.
- Add `crates/kenn-store/SCHEMA_CHANGELOG.md` shipped with the source — one entry per schema bump, documenting what changed and why old snapshots can't be read. Bumping the constant requires adding an entry (enforced by code review, not tooling).
- `get_index_status` surfaces the schema-mismatch case via the existing `failed.error` string (no new wire shape). The error text names both versions so the agent / user knows what reindex would buy them.
- Under `kenn mcp`, schema-stale opens take the existing `Failed → Indexing` recovery path automatically — same `spawn_recovery_pipeline` machinery already used for other Failed states.
- Under `kenn` CLI (e.g. `kenn status`), schema-stale prints the error and exits non-zero so scripts notice. No auto-reindex from the CLI — explicit `kenn index` puts the user in control.

## Capabilities

### New Capabilities

(none — this is a small new requirement on existing capabilities)

### Modified Capabilities

- `store-layout`: add a `Store Schema Version` requirement covering the constant, the per-snapshot persistence, the strict-equality check, and the changelog discipline.

### Affected (no spec change, but implementation touched)

- `mcp-orchestrated-indexing`: schema-mismatch opens transition to `Failed` and trigger the existing recovery pipeline. Add one scenario to the existing "Snapshot freshness check" requirement so the behavior is pinned.

## Impact

- **Code**:
  - `crates/kenn-store/src/lib.rs` — `STORE_SCHEMA_VERSION` constant
  - `crates/kenn-store/SCHEMA_CHANGELOG.md` — initial v2 entry covering the def_range fix
  - `crates/kenn-store/src/{layout.rs, lifecycle.rs}` — write the version into snapshot metadata on publish; read it on open; surface `SchemaMismatch` as a distinct error variant
  - `crates/kenn-mcp/src/indexing.rs` — route `SchemaMismatch` through `Failed` so the existing recovery / `get_index_status` paths handle it for free
  - `crates/kenn-cli/src/cmd_status.rs` — print the mismatch error verbatim; exit non-zero
- **Tools affected (no API change)**: every read tool fails with `INDEX_UNAVAILABLE` when the snapshot is schema-stale, mirroring the existing `Failed` behavior.
- **Data**: existing v1 snapshots are now formally unreadable. Same blast radius as `fix-symbol-def-ranges` — users had to reindex anyway; this just makes the requirement visible.
- **Dependencies**: none.
- **Tests**: a unit test that publishes a snapshot, edits its `schema_version` to `STORE_SCHEMA_VERSION - 1`, reopens, and asserts a `SchemaMismatch` error. An MCP-side integration test that the recovery path fires on schema-mismatch open.
