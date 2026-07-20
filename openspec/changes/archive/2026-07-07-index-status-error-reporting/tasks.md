## 1. Error frames reach the RunReport (jsonl-indexer-driver drift + status)

- [x] 1.1 Extend `UnitCounts` (or a parallel accumulator threaded through `apply_frame`) with a bounded `failed: Vec<String>` filled from `ErrorFrame{severity: error}` (`path`/`source`/`message`, cap 32 + `+N more`) — `crates/kenn-indexer/src/transform_jsonl/stream.rs`
- [x] 1.2 Merge the accumulator into `report.failed_projects` and degrade `report.status` to `Partial` (never overriding `Failed`) in JSONL ingest — `crates/kenn-indexer/src/pipeline/ingest.rs`
- [x] 1.3 Unit tests: error frame → Partial + failed_projects entry; warning frame → Success untouched; cap at 32 with `+N more` tail
- [x] 1.4 Pass the producer name into `record_jsonl_exit_status` and use it in the message instead of the hardcoded `"kenn-dotnet"`; test that a kenn-ts exit names kenn-ts

## 2. Swift sidecar reports build failures on the wire

- [x] 2.1 Emit `ErrorFrame{severity:"error", source:"build", path:<package dir>}` when `runSwiftBuild`/`runXcodebuild` fails, before falling back to reading an existing store — `indexers/kenn-swift/Sources/kenn-swift/Provisioning.swift` + `main.swift` (verify the Rust-side stream state machine tolerates an error frame before `MetaFrame`; if not, emit it right after `MetaFrame`)
- [x] 2.2 Emit an `ErrorFrame{severity:"error"}` when no index store exists for a discovered project (today: stderr log only, project silently skipped)
- [x] 2.3 Integration check: break a fixture package, run the sidecar, assert the error frame appears in the JSONL and (via ingest) yields a `Partial` report with the package in `failed_projects`

## 3. Per-language summary and zero-files warning (index-run-reporting)

- [x] 3.1 Persist per-unit file counts onto `RunReport` (ingest progress events already carry `files`) so the CLI can sum per language — `crates/kenn-indexer/src/report.rs`, `pipeline/ingest.rs`
- [x] 3.2 After `aggregate_status` in `cmd_index.rs`, when aggregate != Success, print one stderr line per non-Success language: `warning: <lang>: <status> — <first failure> (+N more)`; silent on clean runs
- [x] 3.3 Zero-files warning: for each language with ≥1 report and 0 total files, warn naming the language and its claimed extensions (source the extension list from `claimed_extensions` / `Language::extensions()`); no warning when a language produced no reports
- [x] 3.4 Tests: failed-rust-among-others prints rust line only; clean run prints nothing; zero-files warning fires for discovered-but-failed unit and stays quiet for no-sources language

## 4. Verify

- [x] 4.1 `cargo clippy --workspace --all-targets` clean; `just crap-ci` green; `cargo fmt --all` last
- [x] 4.2 End-to-end: index a workspace with one broken producer (e.g. Swift package with a bad build) and confirm `kenn index` prints the summary, `meta.json`/`kenn status` show the failure, and the snapshot still publishes as `partial`

## 5. Review fixes (post-implementation code review)

- [x] 5.1 Structured `failed_overflow` on RunReport/SnapshotMeta replaces the synthetic `+N more` list entry; rendered only at display time (`render_failed_projects`) — counting surfaces (`kenn status`, rollups, regression metric) now see true totals
- [x] 5.2 `ErrorFrame.severity` deserializes as a case-insensitive enum; unknown severities attribute like errors (fail loud); crash-retry exclusion keys on error-severity frames only (warnings precede the crash-prone build phase)
- [x] 5.3 `RunReport.language` (drivers state it via `started_for`) — CLI rolls up by language, deleting the producer-name shadow table; fixes split rollups (false zero-files warnings, duplicate summary lines); css rollup unions Sass extensions
- [x] 5.4 Exit-status failure messages use `report.indexer_name` (stable under runner-form commands); `worse()` replaced by derived `Ord` on RunStatus
- [x] 5.5 jsonl-indexer-driver delta spec: msbuild scenario corrected (workspace diagnostics are pathless; per-entry `indexer` failures carry paths), overflow-as-count clauses

## 6. Third-review fixes

- [x] 6.1 Warnings channel: warning-severity frames recorded on `RunReport.warnings`/`SnapshotMeta.warnings` (bounded + overflow), shown by `kenn status` (`warnings (N):`) and per-language `kenn index` stderr lines — previously they died in a counter, silencing producer staleness notices
- [x] 6.2 `render_failed_projects` generalized to `render_with_overflow` (shared by failures and warnings)
