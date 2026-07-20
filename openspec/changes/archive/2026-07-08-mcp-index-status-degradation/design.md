# Design

## D1 — Cache the summary at snapshot-bind time, never on the call path

`get_index_status` is spec-bound to *no git operations and no store open on
the call path* (mcp-orchestrated-indexing, "Index status reports staleness
and reindex progress"). Reading `meta.json` per status call would be a small
file read, not a store open — but the spirit of that requirement is "cached
state only", and status is called in agent polling loops.

There is exactly **one** site that turns a snapshot path into a reader
binding: `open_binding` → `ReadyParts` in `indexing/orchestrate.rs`. Both
consumers destructure `ReadyParts` — `ready_from_parts` (cold start,
recovery, self-publish reindex completion) and `swap_to_snapshot` (external
`live`-flip rotation). `open_binding` already derives `indexed_at` from the
snapshot path; it reads `<snapshot_path>/meta.json` in the same place. So the
summary is a new `ReadyParts` field threaded to a new `LifecycleState::Ready`
field — one parse site, not four. The parse is off the lifecycle lock (it
happens before the swap takes the write guard), same as the reader open
itself; it is a single small synchronous file read, dwarfed by the
`open_reader` it sits beside.

A missing or unparsable `meta.json` (pre-reporting snapshot, or a
parent-worktree fallback snapshot without one) yields no summary — fields
omitted, exactly like today's payload. `open_binding` reads whatever
`snapshot_path` it is handed, so a parent-worktree fallback correctly reports
the parent snapshot's health.

A **fully-failed** run (`aggregate_status` all-Failed ⇒ abort, no publish)
never produces a served snapshot, so it does not use these fields — it
surfaces via the existing `Failed` lifecycle state (`state: "failed"` +
`error`) or by continuing to serve the prior good snapshot. Only `partial`
(published) and `success` runs reach the Ready payload; do not add
`run_status` to the `Failed` arm.

## D2 — Payload shape: omitted when clean, true counts alongside bounded lists

```json
{
  "state": "ready",
  "run_status": "partial",
  "failed_projects": ["csharp: msbuild: …", "…"],
  "failed_count": 34,
  "warnings": ["swift: 3 stale index-store units kept …"],
  "warning_count": 1
}
```

- `run_status` is `meta.json`'s aggregate status string. Omitted when
  `"success"` **and** there are no warnings — the happy-path payload is
  byte-identical to today, and agents treat field presence as the signal.
- The lists are the bounded attributions persisted in `SnapshotMeta`
  (`JSONL_FAILED_ATTRIBUTION_CAP` per unit); `failed_count` /
  `warning_count` are the true totals (`list.len() + *_overflow`), matching
  how `kenn status` renders `+N more`. Agents get honest counts without
  unbounded payloads.
- No new escalation: a `partial` run still serves and `state` still reflects
  the embed stage. Degradation is reported, not turned into an error.

## D3 — Reuse the existing public reader and renderer; no new struct

`SnapshotMeta` (kenn-indexer) is already public with public fields, and
`kenn_indexer::report::render_with_overflow` is already the shared,
public list-with-`+N-more` renderer that `kenn status` uses. kenn-mcp
depends on kenn-indexer, so it reads the same struct and calls the same
renderer directly — no new type, no parallel meta.json parser.

Concretely, from a parsed `SnapshotMeta`, the payload fields are:
`run_status = meta.status`, `failed_projects =
render_with_overflow(&meta.failed_projects, meta.failed_overflow)`,
`failed_count = meta.failed_projects.len() + meta.failed_overflow`, and the
same two for `warnings`. This is exactly the arithmetic `cmd_status.rs`
already inlines (`len + overflow`); it is a one-line formula, not logic worth
extracting into a shared helper — so **the CLI is not touched** and there is
no cross-crate helper to keep in sync. (The parity directives target the
index-run *orchestration* — build_workspace / configure_runner / the
SnapshotMeta *writer* — not this trivial read-side sum.)

## D4 — Out of scope

- Emitting a notification on degradation (the existing `code_updated`
  notification already fires on rotation; agents poll status after it).
- Surfacing per-run `report.json` details (per-unit diagnostics) over MCP —
  the summary names the failing language/producer; deeper diagnosis is a
  human/CLI task (`kenn status`, `overview.md`).
- Any change to what the pipeline records — done in
  `index-status-error-reporting`.
