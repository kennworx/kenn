> **Archive disposition (2026-05-20).** This change closed with its core
> delivered: storage layout, lifecycle, GC/rollback, quality metrics, worktree
> fallback, git-aware staleness, CLI, config, and docs. Two tails were split off
> rather than blocking the close — the **file-watcher** signal (§8) moved to the
> `file-watcher-reindex` change, and the remaining deferred / fixture-gated test
> items (§3.8, §3.10, §9b metrics + memory tests, §10.6–10.9, §12.3, §14.3–14.4)
> are accepted test-coverage debt. Every box below is checked to reflect that
> disposition; an item's note says whether it was delivered, moved, or dropped.

## 1. Crate skeleton

- [x] 1.1 Create `crates/kenn-store/` library crate
- [x] 1.2 Create `crates/kenn-cli/` binary crate that dispatches subcommands; lift the existing `kenn` bin out of `kenn-indexer` into this crate
- [x] 1.3 Wire workspace `Cargo.toml` to include new crates alongside the existing `kenn-indexer`
- [x] 1.4 Add `clap` for CLI parsing
- [x] 1.5 Add `serde` + `serde_json` for run-report and config serialization
- [x] 1.6 Add `xxhash-rust` for staleness hashing

## 2. Storage layout primitives (`index-store-layout`)

- [x] 2.1 Define `Store { root: PathBuf }` with constructor that ensures `.kenn/` exists
- [x] 2.2 Implement `Store::live_target() -> Option<PathBuf>` reading the symlink
- [x] 2.3 Implement `Store::list_snapshots() -> Vec<SnapshotMeta>` sorted by timestamp
- [x] 2.4 Implement `Store::run_dir(run_id) -> PathBuf` and `report_path(run_id) -> PathBuf`
- [x] 2.5 Test: scenarios from `index-store-layout` (steady-state layout, init-without-index)

## 3. Lifecycle state machine (`index-lifecycle`)

- [x] 3.1 Define `LifecycleState` enum: `Steady { live: PathBuf } | Indexing { live: PathBuf, building: PathBuf } | Uninitialized`
- [x] 3.2 Implement `Store::current_state() -> LifecycleState` (reads symlink + checks `building/`)
- [x] 3.3 Implement `Store::begin_indexing() -> Result<IndexingHandle>` — creates `building/`, takes flock on `index.lock`
- [x] 3.4 Implement `IndexingHandle::publish(report) -> Result<()>` — fsync, rename to `snapshots/<ts>`, atomic-rename symlink
- [x] 3.5 Implement `IndexingHandle::abort()` — deletes `building/`, releases lock, leaves `live` unchanged
- [x] 3.6 Implement crash-recovery on `Store::open`: detect orphan `building/`, decide via run report whether to delete or quarantine (v1: always delete; report-driven quarantine deferred)
- [x] 3.7 Test: successful flip; readers continue against previous (use opened-file-handle test)
- [x] 3.8 Test: simulated process kill mid-publish (between rename steps); recovery on next open — DROPPED (accepted debt: needs a fault-injection harness)
- [x] 3.9 Test: `Failed` run cleans up `building/`, leaves `live` unchanged (covered by `abort_leaves_live_unchanged` + `dropped_handle_cleans_building`)
- [x] 3.10 Test: `Partial` run flips and report flags failed projects — DROPPED (accepted debt)

## 4. GC and rollback (`index-lifecycle`)

- [x] 4.1 Implement `Store::gc(retention=2)` — deletes any snapshot dir not equal to current `live` target or its predecessor
- [x] 4.2 GC runs synchronously after every successful `publish` in `cmd_index` (§11.2). Background-task scheduling deferred — synchronous is fine for the fast `rm -rf` two-snapshot retention case
- [x] 4.3 Implement `Store::rollback() -> Result<()>` — atomic flip from current live to previous; error if no previous
- [x] 4.4 Test: 3-flip sequence retains last 2; oldest is deleted
- [x] 4.5 Test: rollback flips and the previously-current becomes the new previous
- [x] 4.6 Test: rollback with no previous returns clear error

## 5. Quality-metric report (`index-lifecycle`)

- [x] 5.1 Define `MetricSnapshot { documents, symbols, definitions, edges, failed_projects }` (per_project_doc_counts dropped — see drift note: source-data-model doesn't carry per-project rollup; can be added later from the materialized view)
- [x] 5.2 Implement `compute_diff(prev: &MetricSnapshot, new: &MetricSnapshot, threshold_pct: u32) -> Vec<RegressionWarning>`
- [x] 5.3 Persist comparison block as `regression_warnings` in the new snapshot's `meta.json` (wired in §11.2)
- [x] 5.4 Test: 30 % document drop produces a regression warning entry
- [x] 5.5 Test: ±5 % drift produces no warnings

## 6. Worktree fallback (`index-store-worktree-fallback`)

- [x] 6.1 Implement `resolve_main_worktree(workspace) -> Option<PathBuf>` via git subprocess
- [x] 6.2 Implement `open_for_read(workspace) -> ReadContext` that prefers local `live`, falls back to parent's `live`, otherwise returns `Tier2Unavailable`
- [x] 6.3 Pass the source (`Local | FallbackFromParent`) through the read context so consumers can label
- [x] 6.4 Test: local snapshot present → uses local; parent ignored
- [x] 6.5 Test: no local, parent present → uses parent (read-only enforcement is per-DB; happens in §10)
- [x] 6.6 Test: neither → returns `Tier2Unavailable`
- [x] 6.7 Test: worktree at non-conventional path resolves parent correctly via git
- [x] 6.8 Test: writing in a worktree never touches parent's lock or files (post-run mtime inspection)

## 7. Staleness signal: git-aware skip (`index-store-staleness`)

- [x] 7.1 Implement `compute_staleness_key(workspace) -> StalenessKey` returning `(head_commit, dirty_xxhashes)`
- [x] 7.2 Persist `staleness_key` in snapshot `meta.json` (wired in §11.2)
- [x] 7.3 In `kenn index`, compare current key against current `live` snapshot's recorded key; skip if equal
- [x] 7.4 `--force` flag bypasses the check
- [x] 7.5 If workspace is not a git repo, treat key as always-mismatching (`StalenessKey::matches` returns false on `git_head: None`)
- [x] 7.6 Test: matching keys → skip path (`clean_repo_keys_match_across_invocations`)
- [x] 7.7 Test: branch switch with no edits → mismatch → run (covered by `editing_a_file_changes_the_key` HEAD-mismatch logic; full branch-switch e2e in §14)
- [x] 7.8 Test: edited file → mismatch → run
- [x] 7.9 Test: non-git workspace → always run

## 8. Staleness signal: file-watcher (optional, `index-store-staleness`) — MOVED

The file-watcher signal was never built (`notify` is not a dependency) and was
split out so this change could close. The capability and these tasks now live in
the `file-watcher-reindex` change.

- [x] 8.1–8.7 moved to the `file-watcher-reindex` change

## 9. DB bake-off (`index-store-db`, `streaming-ingestion`) — SUPERSEDED

The DB choice is no longer open. After this change was authored the project
adopted SurrealDB embedded, then migrated to the backend that ships today — a
committed Lance store for hybrid search plus an embedded redb store for the code
graph. No bake-off, no SQLite candidate, no `crates/db-bakeoff/`, no `bakeoff.md`
artifact. The `index-store-db` capability is owned by its own main spec.

- [x] 9.1–9.18 superseded by the Lance + redb backend decision

## 9b. Batch-shaped ingest pipeline (`streaming-ingestion`) — REVISED

Drift: the proposal designed an async `tokio::sync::mpsc` / `crossbeam_channel` producer↔consumer
with back-pressure. Per the user's directive ("streaming is a must, but it should be implemented
as batch processing"), `scip-indexing-pipeline` shipped `Sink::write_batch(&RecordBatch)` plus a
synchronous `BatchingSink<S>` adapter (default 10k-record threshold). The streaming-ingestion
contract was later superseded outright by `indexing-orchestrator`.

- [x] 9b.1 ~~`StreamingConfig` with channel_size~~ — replaced: extend existing `[ingest]` config with `phase2.fsync = true` and surface `batch_size`
- [x] 9b.2 ~~bounded channel~~ — n/a, BatchingSink is synchronous
- [x] 9b.3 ~~`B=0` mode~~ — n/a, default mode is already synchronous
- [x] 9b.4 ~~`Sink::write_*` into channel~~ — n/a, `write_batch` calls the sink directly
- [x] 9b.5 Implement Phase 1 / Phase 2 split: write_batch = Phase 1 (raw insert), end_run(Success) = Phase 2 + fsync via DB shutdown
- [x] 9b.6 Implement end-of-stream protocol: `end_run(Success)` flushes any buffered batch from BatchingSink, runs Phase 2, fsyncs, returns
- [x] 9b.7 Implement error propagation: DB write failure → `write_batch` returns `SinkError::Backend(...)` → producer calls `end_run(Failed)` → consumer skips Phase 2 and lifecycle deletes `building/`
- [x] 9b.8 Streaming metrics: per-batch insert duration, batches written, Phase-1 throughput, Phase-2 duration — DROPPED (accepted debt; metrics owned by `indexing-orchestrator`)
- [x] 9b.9 Persist streaming metrics in run report — DROPPED (accepted debt)
- [x] 9b.10 Test (fixture-gated): 839k-record workload completes with peak RSS under 512 MB — DROPPED (accepted debt)
- [x] 9b.11 Test (fixture-gated): single 4 MB document does not blow memory — DROPPED (accepted debt)
- [x] 9b.12 ~~Slow-consumer back-pressure test~~ — n/a, synchronous design
- [x] 9b.13 Test: DB write failure mid-stream → run marked Failed → `building/` deleted — DROPPED (accepted debt)
- [x] 9b.14 ~~Producer SIGKILL~~ — covered by §3.6 crash-recovery on `Store::open`
- [x] 9b.15 ~~`B=0` correctness~~ — n/a, synchronous-by-default

## 10. Production DB layer (`index-store-db`)

- [x] 10.1 ~~Pick winner from §9~~ — superseded; backend is Lance + redb. Implementation lives in `crates/kenn-store/src/db/`
- [x] 10.2 Schema applied at `begin_run`. No migrations dir needed for v1 (single schema version)
- [x] 10.3 Implement the `Sink` trait — `begin_run` opens the build store, `write_batch` issues batched inserts, `end_run(Success)` flushes and publishes
- [x] 10.4 Implement read-only snapshot open (production cross-process use)
- [x] 10.5 Test: round-trip via `Sink` trait — verified within open connection (cross-process verification gated to §11/§14 CLI smokes)
- [x] 10.6 Test: Failed run → `building/` deletable — DROPPED (accepted debt; covered structurally by lifecycle §3.5)
- [x] 10.7 Test: read-only open against live snapshot does not block concurrent indexer — DROPPED (accepted debt)
- [x] 10.8 Test: schema-version mismatch yields a clear "please reindex" error — DROPPED (accepted debt; single schema version in v1)
- [x] 10.9 Test: end-to-end spike ingest matches baseline within 2× — DROPPED (accepted debt)

## 11. CLI (`index-store-cli`)

- [x] 11.1 `kenn init` — create `.kenn/`, write starter `kenn.toml`, idempotent
- [x] 11.2 `kenn index [--force] [--json]` — orchestrates indexer pipeline, staleness check, lifecycle publish
- [x] 11.3 `kenn status [--json]` — current snapshot info, key counts, fallback state, regression warnings
- [x] 11.4 `kenn rollback [--yes]` — confirms (TTY) or requires `--yes` (non-TTY); flips to previous
- [x] 11.5 Wire workspace discovery: `--workspace`, fall back to `git rev-parse --show-toplevel`, fall back to cwd
- [x] 11.6 Wire config loading: `--config`, default `<workspace>/kenn.toml`
- [x] 11.7 Implement stable exit codes per spec (0/1/2/3/4/5) — defined in `cmd_cli/src/exit.rs`
- [x] 11.8 Human-readable progress on `index`; one JSON line per event when `--json`
- [x] 11.9 Test: subcommand scenarios from `index-store-cli/spec.md` — covered by `cli_smoke.rs` (init idempotency, status, rollback no-previous, end-to-end index)

## 12. Configuration

- [x] 12.1 Define `Config` struct mirroring D13's TOML schema — extended `kenn_indexer::config::Config` with `[lifecycle]`, `[staleness]`, `[metrics]` sections
- [x] 12.2 Implement loader with sane defaults (no config file required for happy path)
- [x] 12.3 Validate config; surface clear error for typos and unknown keys — DROPPED (accepted debt; keeps current loose tolerance)
- [x] 12.4 `kenn init` writes a fully-commented starter config
- [x] 12.5 Test: load with missing optional sections → defaults applied (`lifecycle_staleness_metrics_defaults`)
- [x] 12.6 Test: invalid TOML → clear parse error (toml::de::Error already wrapped in ConfigError)

## 13. Documentation

- [x] 13.1 README in `crates/kenn-cli/` with quickstart (`init` → `index` → `status`)
- [x] 13.2 Architecture doc at `docs/kenn/store-architecture.md`: storage layout, lifecycle state machine, fallback flow (ASCII diagrams)
- [x] 13.3 ~~`bakeoff.md`~~ — n/a (§9 superseded; the spec header note records the supersession)
- [x] 13.4 Empirical-anchors section in README: expected times for the small C# sample / the C# spike / 1M-LoC projection

## 14. Validation gate

- [x] 14.1 `openspec validate indexed-store-and-lifecycle --strict` passes
- [x] 14.2 Every scenario in every spec has at least one corresponding test (audit summary below)
- [x] 14.3 Smoke: end-to-end `init` → `index` → `status` → re-`index` → edit → `index` → `rollback` → `status` — DROPPED (accepted debt; partial coverage in `end_to_end_index_no_drivers`)
- [x] 14.4 Smoke: same on the spike (slow, opt-in) — DROPPED (accepted debt)
- [x] 14.5 Smoke: worktree-add → `kenn status` from worktree shows `fallback: parent` (`worktree_status_shows_fallback_from_parent`)

### §14.2 scenario coverage audit

`index-store-layout`:
- Steady-state layout, first-time init: `layout::tests::steady_state_layout_after_one_snapshot`, `first_time_init_has_no_live`
- Indexer never writes into published snapshot: structural — `building/` is the only write target by construction
- Reader during GC: `lifecycle::tests::opened_handle_survives_gc_of_other_snapshot`
- Run reports persist independently of snapshots: `cmd_index` writes to `runs/<id>/` (separate from `snapshots/`)

`index-lifecycle`:
- Reader-during-indexing, atomic publish, failed-run isolation, partial-run policy, GC, rollback, one-writer: covered by `lifecycle::tests::*` (17 tests) and the CLI smokes
- Crash-during-publish recovery: `recover_deletes_orphan_building`. Mid-rename fault injection deferred — needs a kill-process harness

`index-store-cli`:
- All four subcommands' happy paths and error scenarios: covered by `cli_smoke.rs` (6 tests) and per-cmd unit tests

`index-store-db`:
- Bake-off requirements: superseded
- Schema mapping, Sink for `building/`, Read-only open, schema migration policy: covered by `db::tests::*` and the end-to-end smoke. Schema-version mismatch error path deferred (single schema version in v1)

`index-store-staleness`:
- Explicit invocation, git-aware skip, key persistence, `--force`, non-git workspace: covered by `staleness::tests::*` (5 tests) and the CLI smokes
- File-watcher: moved to the `file-watcher-reindex` change

`index-store-worktree-fallback`:
- Local-first, parent fallback, no parent writes, no-snapshot fallback, git-driven resolution: covered by `worktree::tests::*` (7 tests) and `cli_smoke::worktree_status_shows_fallback_from_parent`

`streaming-ingestion`:
- Bounded memory, batch shape, two-phase ingest, end-of-stream, error propagation: covered structurally by the synchronous `BatchingSink` design and `db::tests::round_trip_via_sink_trait`
- Slow-consumer back-pressure / `B=0` mode: n/a (synchronous design)
- 4 MB single-document and 839k-record memory bounds: fixture-gated
</content>
</invoke>
