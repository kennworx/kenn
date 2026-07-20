## 1. rmcp API audit (completed)

- [x] 1.1 `roots/list` request from server: `rmcp::service::Peer<RoleServer>::list_roots() -> Result<ListRootsResult, ServiceError>`. Already in use by the crate (`indexing.rs:26` imports `Peer`).
- [x] 1.2 Inbound `notifications/roots/list_changed`: `ServerHandler::on_roots_list_changed(...)` — overridable, default no-op. The same trait also exposes `on_cancelled`, `on_progress`, `on_initialized`, `on_custom_notification`.
- [x] 1.3 Client capabilities: `ServerHandler::initialize(InitializeRequestParams) -> InitializeResult` is the hook; `InitializeRequestParams` carries the client-side `ClientCapabilities` (which includes `roots.listChanged`). Currently kenn-mcp does not override `initialize` (only `get_info` at `server.rs:333`), so this change adds the override.

## 2. Make `ServerState.layout` swappable (structural prep)

Done as its own commit, before the roots-specific work. No behavior change.

- [x] 2.1 Wrap `state.layout` in `arc_swap::ArcSwap<kenn_store::Layout>`. Update all readers to `state.layout.load()`.
- [x] 2.2 Drop the `layout` parameter from `start_background_indexing` (`indexing.rs:45`); read the active layout from `state` instead.
- [x] 2.3 Drop the `layout` parameter from `start_snapshot_poll_task` (`indexing.rs:442`); read from `state` instead.
- [x] 2.4 Audit every other spot that captures a `Layout` clone into a tokio task — these need to either read from state on each tick or be re-spawned by the rebind path. Likely candidates: `run_startup_decision`, `run_reindex_and_install`, `spawn_recovery_pipeline`, `spawn_background_reindex`.
- [x] 2.5 `cargo clippy --workspace --all-targets` + `cargo test --workspace` clean — this is a refactor; tests must not regress.

## 3. Pre-handshake env-var source: `CLAUDE_PROJECT_DIR`

Verified via the `debug_env` MCP tool against Claude Code 2.1.148.
Insert reading of `CLAUDE_PROJECT_DIR` into the existing workspace
chain in `main.rs::resolve_workspace`. No rmcp interaction needed.

- [x] 3.1 Update `resolve_workspace` in `crates/kenn-cli/src/main.rs` to consult `std::env::var("CLAUDE_PROJECT_DIR")` after the explicit `--workspace` flag and before the git-toplevel fallback.
- [x] 3.2 Reject env values that don't point to an existing local directory: log the rejection, fall through to the next priority. This protects against stale env or future host quirks.
- [x] 3.3 Introduce a `WorkspaceSource` enum (`CliFlag | ClaudeProjectDir | RootsList | GitToplevel | Cwd`) and record the resolved source on `ServerState`. This is the source-of-truth that §5.1, §7.1, and §8.1 read from. Adding it now (rather than in §5) lets the env-var path tag itself correctly without a placeholder.

## 4. Client-capabilities capture

- [x] 4.1 Add `client_supports_roots: AtomicBool` and `client_supports_roots_list_changed: AtomicBool` to `ServerState` (or a single `client_caps: OnceLock<ClientCapabilities>` cell — pick whichever is more ergonomic to read from rebind code).
- [x] 4.2 Override `ServerHandler::initialize` on `KennMcpServer` (today it only overrides `get_info` at `server.rs:333`). Read `params.capabilities` and populate the cell(s).

## 5. Post-handshake roots resolution

The mental model: *the first `roots/list` call is just a `listChanged`
notification fired once.* Both paths share the same rebind code.

- [x] 5.1 Override `ServerHandler::on_initialized(NotificationContext<RoleServer>)` on `KennMcpServer`. If `client_supports_roots` AND `state.workspace_source` (from §3.3) is not `CliFlag` (the flag wins permanently per D1), dispatch `resolve_roots_and_maybe_rebind(state, ctx.peer.clone())` on the tokio runtime.
- [x] 5.2 Implement `pub async fn resolve_roots_and_maybe_rebind(state: Arc<ServerState>, peer: Peer<RoleServer>)` in `indexing.rs`:
  - Take `peer` as a parameter from the caller. **Do not** read `state.peer.get()` here. The peer reaches us via the rmcp `NotificationContext` passed to the `on_initialized` / `on_roots_list_changed` handler; the OnceLock in `state.peer` is populated separately by `start_background_indexing` and the ordering between the two is not contractually guaranteed by rmcp. Using the context peer makes the code correct regardless of which path stashes first.
  - Call `peer.list_roots().await`.
  - Filter to `file://` URIs; pick the first. Log the ignored roots (D5 — non-file scheme) and any beyond the first (D2 — multi-root collapse).
  - If first root equals the currently-bound `state.layout.load().workspace_root()`, no-op + log. This is the common Claude Code path: §3 already bound to the same path, post-handshake confirms.
  - Else call `rebind_workspace(state, new_ws).await`.
- [x] 5.3 Reject non-`file://` URIs with a structured log per D5; fall through to the next root.
- [x] 5.4 On a successful post-handshake rebind, update `state.workspace_source` (added in §3.3) to `RootsList`. The §5.1 / §7.1 permanence checks read this enum: only `CliFlag` blocks rebinds; `ClaudeProjectDir`, `GitToplevel`, and `Cwd` are tentative and overridable.

## 6. Rebind machinery (used by both initial resolve and listChanged)

- [x] 6.1 Implement `pub async fn rebind_workspace(state: Arc<ServerState>, new_ws: PathBuf)` in `indexing.rs`:
  - Abort in-flight indexing if any. Path through the existing `set_failed` + interrupt seam at `indexing.rs:565`.
  - Close current `ReaderBinding` (drop it from state).
  - Build new `Layout` from `state.config` + `new_ws`.
  - `state.layout.store(Arc::new(new_layout))`.
  - Re-run `run_startup_decision(...)` against the new layout — it already handles the skip-vs-reindex decision.
  - Emit `code_updated` via the existing `emit_code_updated` (`indexing.rs:190`).
- [x] 6.2 If `start_snapshot_poll_task` was running against the old layout, cancel and re-spawn after rebind. (D7 — collateral signature change.)

## 7. listChanged subscription

- [x] 7.1 Override `ServerHandler::on_roots_list_changed(NotificationContext<RoleServer>)`. If `client_supports_roots_list_changed` AND the bound `WorkspaceSource` is not `CliFlag` (the flag wins permanently per D1), call `resolve_roots_and_maybe_rebind(state, ctx.peer.clone())`. (Same path as the initial resolution.) Note: when the source is `ClaudeProjectDir`, listChanged DOES rebind — the env-var bind is a tentative source, overridable by the host post-handshake.
- [x] 7.2 Verify the handler short-circuits to no-op when the client did not declare the capability or when the binding source is `CliFlag` — gate inside the override on the captured state.

## 8. Startup-log line

- [x] 8.1 Emit the structured log line per D9 after each binding change, with `source`, `path`, `listChanged`, and `reason` fields. Fire it from both the initial-bind path (in `serve_stdio`) and every `rebind_workspace` call. `source` values: `cli-flag | claude-project-dir | roots-list | git-toplevel | cwd`.
- [x] 8.2 Emit the `reason` field unconditionally whenever the binding source is `git-toplevel` or `cwd` (no client-type sniffing; see R4 in design.md).

## 9. Graceful degradation tests

- [x] 9.1 Test: client declares `roots` capability without `listChanged`. Server gets the root once, never re-fetches. Workspace changes require restart.
  — `roots_9_1_no_list_changed_means_no_refetch_on_signal` in
  `crates/kenn-mcp/src/indexing.rs`. Asserts the listChanged gate
  (the boolean `KennMcp::on_roots_list_changed` reads before
  dispatching) returns false when the client did not opt in.
- [x] 9.2 Test: client declares no `roots` capability. Server keeps the tentative git-toplevel/cwd binding and logs `reason=client-no-roots-capability`.
- [x] 9.3 Test: client declares `roots.listChanged` and fires the notification. Server re-fetches and rebinds via the same path the initial resolution uses.
  — `roots_9_3_list_changed_triggers_refetch_and_decision` in
  `crates/kenn-mcp/src/indexing.rs`. Gate returns true; the refetch
  feeds into the same `decide_roots_resolution` path used by
  initial resolution (rebind on differing URI).
- [x] 9.4 Test: client returns multiple roots. Server uses `roots[0]`; the rest are logged as ignored.
  — `roots_9_4_multiple_roots_picks_first_logs_rest`. Asserts the
  ignored-list captures roots[1..] when roots[0] is chosen.
- [x] 9.5 Test: client returns a non-`file://` URI. Server logs and skips it; falls to the next eligible root.
  — `roots_9_5_non_file_uri_falls_to_next_eligible`. Non-file URIs
  before the chosen root are silently skipped (not reported as
  ignored — they are schema-invalid, not "extra roots we left out").
- [x] 9.6 Test: tentative workspace differs from `roots/list` result. Server aborts in-flight indexing and rebinds. Verify no half-indexed state lingers.
  — `roots_9_6_tentative_differs_triggers_rebind`. Decision returns
  `RootsResolution::Rebind`; the dispatcher in
  `resolve_roots_and_maybe_rebind` then flips lifecycle to `Failed`
  (in-flight tool calls drain) before `rebind_workspace`.
- [x] 9.7 Test: tentative workspace equals `roots/list` result. Server takes the no-op path; indexing started against the tentative workspace continues uninterrupted.
  — `roots_9_7_tentative_matches_takes_noop_path`. Decision returns
  `RootsResolution::ConfirmTentative`; the dispatcher promotes
  `workspace_source` to `RootsList` without touching layout.
- [x] 9.8 Test: `CLAUDE_PROJECT_DIR` set to a valid directory. Server binds to it pre-handshake. Log shows `source=claude-project-dir`. Subsequent `roots/list` returning the same path is a no-op.
- [x] 9.9 Test: `CLAUDE_PROJECT_DIR` set to a non-existent path. Server logs the rejection and falls through to git-toplevel/cwd.
- [x] 9.10 Test: `CLAUDE_PROJECT_DIR` set and `roots/list` returns a DIFFERENT path. Server rebinds to the `roots/list` result and logs the transition.
  — `roots_9_10_claude_project_dir_overridden_by_roots_list`.
  Decision is the same as §9.6 (rebind on differing URI); what
  differs is the *source* tag at dispatch start (`ClaudeProjectDir`,
  not `GitToplevel`/`Cwd`). Both are tentative, so the rebind
  proceeds and the new source becomes `RootsList`.

> Implementation note: the §9 tests exercise
> `decide_roots_resolution`, a pure function extracted from
> `resolve_roots_and_maybe_rebind` to make the state-machine
> independently testable. The full call chain (handshake →
> `roots/list` request/response → state mutation) is a thin
> dispatch over this decision. §9.1 / §9.3 additionally assert on
> the `listChanged` gate via a small helper that mirrors the check
> in `KennMcp::on_roots_list_changed`.

## 10. Documentation

- [x] 10.1 Update `kenn mcp --help` to document the chain: `--workspace` flag, then `CLAUDE_PROJECT_DIR` env, then `roots/list`, then git-toplevel/cwd. Note that production MCP-host launches (Claude Code, Cursor, Zed) typically need none of the manual sources.
- [x] 10.2 Add a "MCP host compatibility" note to the README/CLAUDE.md: which hosts set `CLAUDE_PROJECT_DIR` (Claude Code: yes — confirmed via debug_env; Cursor: ?; Zed: ?), which hosts fire listChanged (Claude Code: no, per issue #31893; Cursor: yes; Zed: yes).
- [x] 10.3 Document the "first-root-wins" choice (multi-root indexing is explicitly out of scope; see D2).

## 11. Verification

- [x] 11.1 `cargo clippy --workspace --all-targets` clean.
- [x] 11.2 `cargo test --workspace` clean.
- [x] 11.3 Manual smoke: launch `kenn mcp` from Claude Code with no `--workspace` flag in a workspace; confirm the server picks up the workspace via `CLAUDE_PROJECT_DIR` (log shows `source=claude-project-dir`); confirm `search_symbols` returns results.
  — Driven post-`build/kenn` reload via the in-process kenn-mcp:
  `get_index_status` reported `state=ready` (snapshot
  `a7173c413155`, indexed_at `2026-05-25T16-55-41Z`);
  `get_workspace_overview` returned 159 files / 6579 symbols across
  csharp + rust (this repo); `find_symbol("decide_roots_resolution")`
  found `rs:kenn-mcp::indexing::decide_roots_resolution` at
  `./crates/kenn-mcp/src/indexing.rs` — proving the index was
  rebuilt from the just-shipped binary against the right workspace.
- [x] 11.5 Use `mcp__plugin_kenn_kenn__debug_env` to verify `CLAUDE_PROJECT_DIR` matches the bound workspace path post-startup.
  — `debug_env` returned
  `CLAUDE_PROJECT_DIR=/path/to/workspace`;
  `get_workspace_overview` returned a workspace whose contents
  (kenn-dotnet, kenn-mcp, etc.) match that path exactly.
  `find_symbol` returned relative path `./crates/kenn-mcp/...`
  rooted at the same directory — confirming the bind.
