# index-status-error-reporting — design

## Context

Three reporting gaps found while auditing how `kenn index` behaves on non-compiling targets:

1. `Frame::Error` handling in `crates/kenn-indexer/src/transform_jsonl/stream.rs:61-63` discards severity, path, source, and message (`counts.errors += 1`). The `jsonl-indexer-driver` spec ("One run report per invocation") already requires `failed_projects` to be populated from `ErrorFrame{severity: error}` paths — this is implementation drift, and it makes C#/TS project-load failures invisible in `kenn status`.
2. The Swift sidecar logs a failed `swift build`/`xcodebuild` to stderr and exits 0 (`Provisioning.swift:41-50`, `main.swift:62-81`), so the Rust side records `Success` while ingesting a stale or absent store.
3. `kenn index` prints only `published → …` on partial runs; an enabled-but-failed language leaves its files absent (claimed extensions are skipped by the text fallback) with no signal anywhere.

Incidental: `record_jsonl_exit_status` (`pipeline/ingest.rs:~476`) hardcodes `"kenn-dotnet"` into the failure message for every JSONL producer.

## Goals / Non-Goals

**Goals:**
- Error frames with `severity: error` reach `RunReport.failed_projects` and degrade the unit's status to `Partial`.
- A failed Swift provisioning build is visible as a structured error frame, hence in `failed_projects` and status.
- `kenn index` prints a per-language outcome summary when the run is not fully `Success`.
- An enabled language that produced zero files despite having units triggers a warning naming the claimed extensions.
- Exit-status failure messages name the actual producer.

**Non-Goals:**
- Mitigating Swift build failures themselves (continue-after-errors, staleness detection) — separate change `swift-build-error-tolerance`.
- Outcome-aware text-fallback routing (indexing failed languages' files as text).
- Changing `aggregate_status` semantics or process exit codes. (meta.json gained additive `#[serde(default)]` fields — `failed_overflow`, `warnings`, `warnings_overflow` — during review fixes; old snapshots still read.)

## Decisions

### D1 — Error-frame attribution accumulates in `JsonlIngestStats`, not via `&mut RunReport`

`handle_frame` already threads the stats; extend it with a bounded `failed: Vec<String>` (formatted from the frame's `source`/`path`/`message`, capped at 32 entries) filled only for error-severity frames. Ingest merges it into `report.failed_projects` and sets `RunStatus::Partial` (never overriding an existing `Failed`). Attributions past the cap are recorded as a **structured `failed_overflow` count** on the report — never a synthetic `"+N more"` list entry, which counting consumers (rollups, `kenn status`, regression metrics) would mistake for a real failure (review finding). Display surfaces render the overflow via `render_failed_projects`. The cap prevents a pathological project (per-file msbuild errors) from ballooning `meta.json`.

### D2 — Severity is a parse-time enum; warnings don't affect status

`ErrorFrame.severity` deserializes into a `Severity` enum (case-insensitive; `warn`/`warning` → Warning; anything unrecognized → Other, which consumers treat like an error — an unknown severity must fail loud, not silently lose attribution). Only Warning is status-neutral; Error/Other degrade to `Partial`. Warnings are NOT dropped (third review: they died in a counter, silencing the Swift staleness notices): they land on `RunReport.warnings`/`SnapshotMeta.warnings` (bounded + structured overflow) and surface via `kenn status` and a per-language `kenn index` stderr line. This matches the sidecars' intent: kenn-dotnet emits msbuild load failures as errors and diagnostics as warnings.

### D3 — Swift sidecar emits the build failure as an error frame, then continues

`ensureSwiftPMStore`/`ensureXcodeStore` currently return only the store path; on build failure they emit `ErrorFrame{severity: "error", source: "build", path: <package dir>, message: "swift build failed; reading any existing store"}` on the JSONL channel (stdout) before the existing fallback-to-existing-store behavior. The no-store-at-all case also emits an error frame (today: stderr only; the store-*open* failure already emits a frame — `Indexer.swift:63-74`). The sidecar still exits 0: partial output remains valid, and the Rust side's new D1 handling degrades the unit to `Partial`. Alternative — non-zero sidecar exit — would conflate "produced nothing" with "produced a degraded stream" and lose the per-package attribution.

### D4 — Per-language summary grouped by the report's language field

`RunReport` carries a `language: Option<Language>` — derived from db-name `indexer_name`s in `started()`, stated explicitly by branded drivers via `started_for(Language, ...)`. The CLI rolls reports up by that language (falling back to the raw producer name for language-less auxiliary units like `html-resolve`) and prints one stderr line per non-`Success` language: `warning: <lang>: <status> — <first failed_projects entry> (+N more)`. Grouping by language (not `indexer_name`) is load-bearing: one language emits reports under two names — branded per-unit reports and language-id failure reports from `failed_unit_report` — and grouping by name split them, producing duplicate lines and false zero-files warnings (review finding). Carrying the language on the report also deletes the CLI's producer-name→language shadow table, which silently missed new drivers.

### D5 — Zero-files warning keyed on "degraded and produced no files"

After ingest, for each language whose reports include any non-`Success` status, sum `files_seen` across its reports; if 0, print `warning: <lang> indexed 0 files — <exts> files are absent from the snapshot`. Keying on "≥1 report" alone (the first draft) false-positives: JSONL producers (kenn-swift/kenn-dotnet/kenn-ts) run once per workspace unconditionally, so an enabled language with no sources still yields one `Success` 0-files report. Requiring a degraded status catches the real blackhole (units existed, semantic pass failed, files absent) and stays quiet for intentionally empty languages. `RunReport.files_seen` already carries the per-unit file count — no new plumbing.

### D6 — exit-status messages name the report's producer

`record_jsonl_exit_status` reads `report.indexer_name` for the failure message — the branded producer name the driver set at construction. No extra parameter: the report is already in hand, and `indexer_name` stays correct under runner-form command configs (`["dotnet", "kenn-dotnet.dll"]`) where a binary-path-derived label would read `dotnet`.

## Risks / Trade-offs

- [Duplicate attribution] A failing project can appear in `failed_projects` twice — once from its error frame, once from the exit-status path (e.g. kenn-dotnet exits 1 when nothing was produced). → Acceptable: entries are strings with different prefixes; dedup adds complexity for a cosmetic issue.
- [Noise on large broken solutions] Many error frames → capped at 32 + `+N more` (D1).
- [Swift stdout discipline] The build runs before the `MetaFrame` is emitted today; emitting an error frame first must not confuse the Rust-side stream state machine. → Verify frame-order tolerance in `transform_jsonl` (meta-before-frames assumption); if strict, emit the build error frame right after `MetaFrame` instead — the sidecar knows the failure before it starts walking units.
- [Warning fatigue] D4/D5 both print on every degraded run. → Both are single-line-per-language and only on non-Success runs; `kenn status` remains the detailed view.

## Open Questions

- ~~Should the zero-files warning also be persisted into `meta.json`?~~ Partially resolved: meta.json now carries producer `warnings` (third review — they were dying in a counter). The zero-files warning itself stays stderr-only: it is derived from per-language file counts at display time and `failed_projects` captures the underlying cause.
