## Why

`index-status-error-reporting` (archived 2026-07-07) made degraded index runs
honest on the CLI surfaces: per-language failures land in
`SnapshotMeta.failed_projects`/`failed_overflow`, status-neutral producer
diagnostics in `warnings`/`warnings_overflow`, and both `kenn status` and
`kenn index` stderr render them. But the **primary consumer of index health is
an agent driving the MCP server**, and the MCP surface still reports none of
it: `get_index_status` / `wait_for_index` return `state`, `snapshot_id`,
`is_stale`, embed-stage info — nothing about whether the *snapshot itself* was
built from a partial run. An agent whose C# sidecar failed sees `state:
"ready"` and silently searches a graph missing a language. The data is already
persisted per snapshot (`meta.json`, written identically by both index paths);
this change only surfaces it.

## What Changes

- `IndexStatus` (returned by both `get_index_status` and `wait_for_index`,
  which share `build_index_status`) gains the degraded-run report of the
  **served snapshot**:
  - `run_status`: the aggregate run status from `meta.json`
    (`"success" | "partial" | "failed"`),
  - `failed_projects` + `failed_count`: the bounded attribution list and the
    true total (list length + overflow),
  - `warnings` + `warning_count`: same shape for status-neutral diagnostics.
  Fields are omitted from the JSON when the run was clean (`success`, no
  warnings), so the happy-path payload is unchanged.
- The report is **parsed once per reader binding**, not read per call:
  `get_index_status` is spec-bound to "no git operations and no store open on
  the call path" (mcp-orchestrated-indexing). There is a single site that
  binds a snapshot — `open_binding` in `indexing/orchestrate.rs`, feeding both
  cold-start/recovery/self-publish and external `live`-flip rotation — which
  reads `<snapshot>/meta.json` into the existing public `SnapshotMeta` (the
  same struct `kenn status` reads) and carries it on the reader binding.
  kenn-mcp reads `SnapshotMeta`'s public fields and reuses the existing public
  `render_with_overflow`; it adds no new struct and does not touch the CLI.

## Capabilities

### Modified Capabilities

- `mcp-orchestrated-indexing`: `get_index_status` (and `wait_for_index`)
  additionally report the served snapshot's degraded-run summary
  (run status, bounded failed-project and warning lists with true counts),
  sourced from the snapshot's persisted metadata via a rotation-time cache —
  never a call-path store read.

## Impact

- **Code, entirely within kenn-mcp:** `indexing/orchestrate.rs`
  (`open_binding`/`ReadyParts` parse `meta.json`), `state.rs`
  (`LifecycleState::Ready` gains the summary), `types.rs` (`IndexStatus`
  fields), `tools/lifecycle.rs` (`build_index_status` populates them).
  Reused as-is: `kenn_indexer::SnapshotMeta` and
  `kenn_indexer::report::render_with_overflow` (both already public;
  kenn-mcp already depends on kenn-indexer).
- **Wire:** additive, optional JSON fields — existing agents unaffected.
- **Not in scope / not touched:** the pipeline recording (done in
  `index-status-error-reporting`), the CLI (`kenn status` already renders
  this; no shared helper is extracted — the read-side count is a one-line
  `len + overflow`), and MCP error semantics — a `partial` run still serves
  (`state` stays `ready`/`embedding`; degradation is reported, not escalated).
