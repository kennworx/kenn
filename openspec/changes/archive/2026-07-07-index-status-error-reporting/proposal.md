# index-status-error-reporting

## Why

A `kenn index` run can silently degrade: the Swift sidecar exits 0 after a failed `swift build` (report reads `Success` while a stale or empty store was ingested), C#/TS sidecar error frames are discarded down to a bare counter, and a language whose semantic pass fails leaves its files absent from the snapshot with no warning. The `jsonl-indexer-driver` spec already requires error frames to populate `failed_projects` — the implementation drifted (`Frame::Error(_) => counts.errors += 1` throws away severity, path, and message). Users need `kenn index` and `kenn status` to tell the truth about partial coverage.

## What Changes

- Promote JSONL `ErrorFrame{severity: error}` frames into the unit's `RunReport`: populate `failed_projects` from the frame `path`/`message` (closing the existing spec drift) and degrade `report.status` to `Partial` when any are present.
- Swift sidecar emits a structured `ErrorFrame` when `swift build` / `xcodebuild` fails (today: stderr log only, exit 0, report `Success`) — the build failure becomes visible in `failed_projects` and status.
- `kenn index` prints a per-language summary on non-`Success` runs (e.g. `swift: partial — build failed for <pkg>`), instead of only `published → …`.
- Warn when an enabled language produced zero files: its extensions are claimed (so the text fallback skips them) yet nothing was indexed — the claimed-extension blackhole becomes a visible warning instead of silent absence.
- Fix the hardcoded `"kenn-dotnet"` label in the shared JSONL exit-status message (`record_jsonl_exit_status`) so failures name the producer that actually exited non-zero.

## Capabilities

### New Capabilities

- `index-run-reporting`: user-facing reporting of per-language index run outcomes — the per-language summary printed by `kenn index` on partial runs, and the enabled-language-produced-zero-files warning.

### Modified Capabilities

- `jsonl-indexer-driver`: the "One run report per invocation" requirement gains status semantics — a stream containing `ErrorFrame{severity: error}` MUST degrade the report to `Partial` (in addition to the already-required `failed_projects` attribution, which the implementation must actually honor).
- `swift-stream-indexer`: a failed provisioning build (`swift build` / `xcodebuild`) MUST emit an `ErrorFrame` naming the package/project before falling back to reading any existing store; exit 0 with a stderr-only log is no longer conformant.

## Impact

- `crates/kenn-indexer/src/transform_jsonl/stream.rs` — `Frame::Error` handling carries severity/path/message up to the report instead of only counting.
- `crates/kenn-indexer/src/pipeline/ingest.rs` — thread error-frame attribution into `RunReport`; fix producer label in `record_jsonl_exit_status` (~line 476).
- `crates/kenn-cli/src/cmd_index.rs` — per-language summary output on partial runs (~line 219); zero-files warning.
- `indexers/kenn-swift/Sources/kenn-swift/Provisioning.swift` + `main.swift` — emit build-failure `ErrorFrame` (frame plumbing already exists for store-open failures).
- `kenn-dotnet` / `kenn-ts` sidecars already emit error frames — no sidecar changes needed there.
- Downstream: `meta.json` `failed_projects` becomes more populated; `kenn status` output unchanged in shape, richer in content. Existing snapshots unaffected.
