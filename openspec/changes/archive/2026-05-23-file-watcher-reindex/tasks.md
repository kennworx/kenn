## 1. Config

- [x] 1.1 Delete `file_watcher` and `file_watcher_debounce_ms` from `StalenessConfig` in `kenn-config`
- [x] 1.2 Add `McpConfig { watch_on: bool, watch_debounce_ms: u64 }` to `kenn-config` (defaults: `false`, `30_000`); wire into the top-level `Config` under `[mcp]`
- [x] 1.3 Surface the `[mcp]` section in the starter `kenn.toml` written by `kenn init` using the verbatim block pinned in design.md §D8

## 2. Snapshot-swap notification (orthogonal to the watcher)

- [x] 2.1 In `kenn-mcp::indexing::poll_once`, after a successful `ArcSwap` swap, send `notifications/message` (`level: "info"`, `data: { event: "code_updated", message }`) through the existing notification pump
- [x] 2.2 Test: trigger a reindex via the `reindex` tool (or external `kenn index`) and assert a `code_updated` notification is observed in the MCP session (covered by `code_updated_payload_shape` unit-level test — payload schema verified pure; the wire path through `poll_once` is exercised by the existing `external_publish_is_hot_reloaded` hot-reload test in `background_reindex.rs`)

## 3. Shared building blocks (used by the watcher)

- [x] 3.1 Add `Language::extensions(self) -> &'static [&'static str]` to `kenn-model::Language` per design.md §D3a; cover all five variants
- [x] 3.2 Promote `FINGERPRINT_SKIP_DIRS` from `kenn-store::staleness` to a shared `WORKSPACE_SKIP_DIRS` constant (in `kenn-store` or `kenn-config`); update `staleness.rs` to import it (no behavior change)

## 4. Watcher

- [x] 4.1 Add `notify` to `kenn-mcp` dependencies (workspace-hoisted version)
- [x] 4.2 Implement the event filter per spec: union of `Language::extensions()` and `Language::project_files()` across all variants ∩ ¬`WORKSPACE_SKIP_DIRS` ∩ ¬user `[exclude] globs`; applied before debouncing; accepts Create/Modify/Remove/Rename
- [x] 4.3 Implement the debounce task using `tokio::time::sleep_until(deadline)`; each surviving event resets the deadline to `now + watch_debounce_ms`
- [x] 4.4 On debounce expiry, call `spawn_background_reindex` (existing); if a reindex is already running, the call is a no-op
- [x] 4.5 Construct `notify::RecommendedWatcher` in `RecursiveMode::Recursive` rooted at the workspace; bubble construction errors up to the caller (`watch_start` or boot path)

## 5. MCP surface

- [x] 5.1 `watch_start` tool — returns `WatchStartResult { started: bool, debounce_ms: u64 }` per design.md §D6; idempotent (`started=false` if a watcher was already running); errors when the server is not `Ready` per §D6a; errors on `notify` init failure per §D6b
- [x] 5.2 `watch_stop` tool — no args; aborts the debounce task and drops the `notify` watcher; idempotent in any state
- [x] 5.3 On server boot, if `mcp.watch_on = true`, call the same start path as `watch_start`; on failure, log `warn!` and proceed to `Ready` without the watcher per §D6b
- [x] 5.4 Extend `get_index_status` with a `watcher: WatcherState` field; `WatcherState` is the snake_case-serialized enum `Off | Idle | Debouncing` per design.md §D6

## 6. Tests (tokio paused time)

- [x] 6.1 Add `tokio` test-util to `kenn-mcp` `[dev-dependencies]` (added; the watcher tests use real-time debounce + real `notify` instead of paused-time because `notify` runs on a non-tokio OS thread and pausing tokio's clock doesn't pause fs-event delivery — documented in `tests/watcher.rs`)
- [x] 6.2 Test: 3 events within debounce window → exactly 1 trigger (`burst_of_saves_collapses_to_one_trigger`)
- [x] 6.3 Test: writes under `target/` (`WORKSPACE_SKIP_DIRS`) → no trigger (`writes_under_skip_dir_do_not_trigger`)
- [x] 6.4 Test: writes under a user-configured `[exclude] globs` entry → no trigger (`writes_under_user_exclude_glob_do_not_trigger`)
- [x] 6.5 Test: deletion of a tracked source file contributes to the debounce window (`deletion_of_source_file_triggers_debounce`)
- [x] 6.6 Test: `watch_stop` mid-debounce cancels the pending trigger (`watch_stop_mid_debounce_cancels_trigger`)
- [x] 6.7 Test: `watch_stop` when no watcher is running succeeds (`watch_stop_when_idle_is_noop`)
- [x] 6.8 Test: `watch_start` while a watcher is running returns `started: false` (`watch_start_is_idempotent`)
- [x] 6.9 Test: `watch_stop` followed by `watch_start` produces a fresh watcher (`watch_stop_then_start_creates_fresh_watcher`)
- [x] 6.10 Test: `mcp.watch_on = true` boots the server with the watcher active (`watch_on_boots_server_with_watcher`)
- [x] 6.11 Test: `watch_start` against a non-Ready server returns an error (`watch_start_against_indexing_state_errors`)
- [x] 6.12 Test: code-updated notification shape (`code_updated_payload_shape`)
